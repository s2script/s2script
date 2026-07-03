# Slice 5D.1 — live-gate findings (game events)

**Server:** Docker CS2 (`s2script-cs2`), de_inferno, `bot_quota 2`.
**Core+shim:** sniper-built with the 5D.1 event mechanism.

## Outcome: mechanism + typed catalog PROVEN; live event *delivery* BLOCKED by CS2 (deferred)

The whole event subsystem is built, reviewed, and proven in-isolate (T1 core mechanism 115 tests, T2
`@s2script/events` module 116 tests, T4 codegen 27 tests). At the live gate the shim loads and the plugin
subscribes, but CS2 does not export the game-event *manager*, so events can't be delivered live yet.

## Live blocker #1 (FIXED): the shim wouldn't load

`[META] Failed to load plugin s2script: undefined symbol: MurmurHash2LowerCase(char const*, unsigned int)`.

Reading an `IGameEvent` field by name constructs a `CKV3MemberName(key)`, whose ctor → `MakeStringToken` →
`MurmurHash2LowerCase` (tier1, `hl2sdk/tier1/generichash.cpp`). The shim links no tier1, so the symbol was
unresolved at `dlopen`. Compiling `generichash.cpp` (or linking `tier1.a`) cascades into
`CUtlString`/`UtlVectorMemory`/`V_tier0_strlen`. Since `CKV3MemberName` needs ONLY that one function, we provide
it **self-contained** (`shim/src/tier1_shims.cpp`: Valve's exact `MurmurHash2` core + a plain ASCII lowercase in
place of `CUtlString::ToLowerFast`). **Result:** the shim loads; core inits; `[demo] onLoad (game events)` runs
and `Events.on(...)` subscribes. Observed the full boot: `interface OK: Source2Server / EngineCvar /
NetworkServerService / SchemaSystem / GameResourceService`, `Load(): initializing V8 core`, `@s2script/cs2
registered`, `[demo] onLoad`.

## Live blocker #2 (DEFERRED — a signature-scan follow, 5D.1b): the manager isn't exported

`[s2script] WARN: interface MISSING: GameEventManager (GAMEEVENTSMANAGER002) — game-event natives degrade`.

`IGameEventManager2` is **not exposed via `CreateInterface` in CS2** — neither `GetServerFactory` nor
`GetEngineFactory` resolves `GAMEEVENTSMANAGER002` (a `CreateInterface` factory returns null for an unregistered
string regardless of timing, so this is not a "too early in `Load()`" issue). This is a known CS2 reality:
CounterStrikeSharp / Swiftly obtain the manager via a **signature scan** (a byte-pattern match against the game
module for the manager global). That is engine-RE + a per-CS2-update-fragile gamedata signature — the treadmill
work deferred for this slice.

**Degrade behavior (verified):** with the manager null, `event_subscribe` is a no-op (no `AddListener`), so no
events are delivered and the accessors would return defaults — the framework runs normally, no crash. The
`Events.on` API + the typed `GameEvents` overlay are fully usable; they simply receive nothing until the
manager is acquired.

## What 5D.1b needs

A signature-scan acquisition of `IGameEventManager2` (its global pointer in the game module), stored as a
regenerable **gamedata signature** (per the layout-is-data / treadmill discipline). Once `s_pGameEventManager`
is non-null, the existing shim listener + core dispatch deliver events with no further changes — the mechanism
is done. The live gate (an event fires → JS handler → `Player.fromSlot(ev.getPlayerSlot(...))` → fields marshal)
runs then.
