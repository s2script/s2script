# Slice 5B.4 — live spike + gate findings

**Server:** Docker CS2 (`s2script-cs2`), de_inferno, `bot_quota 2` (bots `Specialist`, `Rex`).
**Core:** sniper-built with the 5B.4 natives (`libs2script_core.so` GLIBC ≤ 2.30, `s2script.so` GLIBC 2.14).
**Plugin:** `examples/demo-plugin` reading identity two ways per in-game player and comparing.

## Spike — raw primitives validate the char[N] layout + the uint64 read

Resolved live via `__s2_schema_offset("CBasePlayerController", …)`:
- `m_iszPlayerName` → offset **2036** (a `char[128]` inline buffer).
- `m_steamID` → offset **2528** (a `uint64`).

Raw reads on the controller `EntityRef`:
- `ref.readString(2036, 128)` → `"Specialist"` / `"Rex"` — the inline `char[N]` buffer reads correctly, NUL-terminated, no garbage.
- `ref.readUInt64(2528)` → `0` (a `bigint`) for both bots — bots have steamid64 `0`; a real player would read `7656…`. The read path is exercised regardless (a non-crashing, serial-gated 64-bit read).

**Conclusion:** `char[N]` is a direct inline byte read (no pointer/table deref, unlike `CUtlSymbolLarge`/`CUtlString`); `uint64` is a direct 8-byte read. Both primitives are correct live.

## Gate — generated accessors agree with the spike

`Player.all()` → the 2 bots. Per player, the **generated** accessors:
- `player.playerName` → `"Specialist"` / `"Rex"` — **matches** the raw `readString`. (char[128] → string)
- `player.steamID` → `"0"`, and **`typeof player.steamID === "string"`** — the load-bearing decision confirmed live: a `uint64` field generates a **decimal string** (SM-parity, wire-safe), not a `bigint`. Matches the raw `readUInt64(...).toString()`.
- `player.pawn.health` → `100`.

Sample log (before kick):
```
[demo] tick 257 players=2
  slot=0 | GEN name="Specialist" steamID=0 (typeof string) | RAW name="Specialist" sid=0 (nameOff=2036 sidOff=2528) health=100
  slot=1 | GEN name="Rex"        steamID=0 (typeof string) | RAW name="Rex"        sid=0 (nameOff=2036 sidOff=2528) health=100
```

## Disconnect degrade + liveness

`bot_kick` → `players=0` from the next demo tick on (the 5C.2 occupancy filter drops the now-pawnless controllers; the generated accessors would read `null` on a stale ref). The server kept ticking well past the kick (tick 3585 → 4865+), no crash, no segfault; rcon `status` → `0 bots, not hibernating`.

**Result:** spike + gate both PASS. `char[N]` strings + `uint64`→decimal-string generated accessors read correct live, degrade on disconnect, server stable.
