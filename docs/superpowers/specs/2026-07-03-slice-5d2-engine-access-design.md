# Slice 5D.2 — engine-backed access: live game events + engine identity (design)

**Goal:** Light up live game-event delivery (the deferred 5D.1b) *and* engine identity
(`player.userId`, `Player.fromUserId`, connected-but-pawnless enumeration), by reaching two
un-exported CS2 engine facilities through **committed, regenerable gamedata facts** — one signature
and a handful of offsets — with the access mechanism engine-generic in core/shim and the facts +
player API in the CS2 game layer.

**Status:** design approved; feasibility proven offline by two RE spikes (see
`2026-07-03-slice-5d1b-sigscan-spike-findings.md` — the event-manager signature RECIPE-FOUND high
confidence, and the engine-identity offsets PARTIAL/usable-core high confidence). This spec is the
authority for the exact values; the implementation plan turns it into tasks.

**Branch base:** `main` (0–5A + entref-wire + 5B + 5C.1/5C.2/5B.4/5C.3/5C.4/5C.5 + 5D.1 merged).
**Cadence:** subagent-driven, merge-to-main-locally, live Docker CS2 gate.

---

## 1. Framing — what's new vs. reused

Two independent vertical threads sharing one "layout-is-data" spine.

- **Thread A (events):** reuses ALL of 5D.1 verbatim — the shim's `IGameEventListener2`
  (`S2ScriptEventListener`), `s_currentEvent` save/restore, `core/src/event_mux.rs`, the 8 event
  `S2EngineOps`, the `__s2_event_*` natives, `Events.on` + the `GameEvent` accessor, and the typed
  `events.generated.d.ts` catalog. The ONLY thing 5D.1 lacked was a non-null `s_pGameEventManager`
  (the factory returns nothing — confirmed at the binary level: `GAMEEVENTSMANAGER002` is in zero
  modules). Thread A supplies that pointer via a signature scan and changes nothing else.
- **Thread B (identity):** entirely new — an engine-generic client-list reader in the shim, new
  engine ops + natives, and CS2 JS/​types for `player.userId` / `Player.fromUserId` /
  `Player.allConnected()`.

Both threads' engine facts live in `gamedata/core.gamedata.jsonc`; both mechanisms live in
engine-generic core/shim. Neither leaks a CS2 identifier into `core/src` (boundary gates stay green).

---

## 2. The spine — gamedata facts (layout is data)

Every value the RE produced is DATA, committed and regenerable per patch from documented anchors.
No pattern/offset is hardcoded in code. **All of it lives in the ONE existing gamedata file**,
`gamedata/core.gamedata.jsonc` (repo root; JSONC — comments allowed), which the shim already reads
via `GamedataPath()` for `.interfaces` and `.offsets`. This slice adds a `.signatures` section and
new `.offsets` keys to that same file (packaged into the addon by `package-addon.sh`, which already
copies it). Concrete loader facts from the code: `LoadOffsets` reads `.offsets.<key>.<platform>` and
returns `map<string,int>` via `.get<int>()` — so **offset values are DECIMAL integers** (JSON has no
hex literal; hex goes in a trailing `//` comment), and **dotted keys** (`"Class.field"`) are valid
JSON object keys returned verbatim.

### 2.1 New: a `.signatures` section + a `LoadSignatures` loader

Add to `gamedata/core.gamedata.jsonc`:

```jsonc
"signatures": {
  "GameEventManager": {
    "linuxsteamrt64": {
      "module": "libserver.so",
      "pattern": "55 48 8D 05 ? ? ? ? BE 40 00 00 00 48 89 E5 41 54 4C 8D 65 E8 53 48 89 FB",
      "resolve": "ctor-body-xref"
    }
  }
}
```

- New shim loader `LoadSignatures(path, platform, error)` mirroring `LoadOffsets` (same JSONC parse,
  `.signatures.<key>.<platform>` → `{module, pattern, resolve}`; absent section is not an error).
- `pattern` — IDA-style byte pattern; `?` (or `??`) is a wildcard byte. This is the **ctor-body**
  signature (the more update-stable of the two the spike found — a function body survives register
  churn better than a single call site). Unique: 1 match in `libserver.so` `.text`.
- `resolve: "ctor-body-xref"` names the extraction strategy the scanner applies (see §3.1): find the
  matched ctor's address, find the single `E8` rel32 call whose target is the ctor, walk back to the
  nearest `lea r14/r13,[rip+d]`, compute `instance = lea_addr + 7 + d`.
- The simpler primary call-site signature
  (`4C 8D 35 ? ? ? ? E8 ? ? ? ? 4C 89 F7 E8 ? ? ? ?`, `resolve: "lea-disp"`, `disp @ +3`, lea len 7)
  is retained in the spec as the documented alternate; the plan implements `ctor-body-xref` as the
  shipped strategy and MAY additionally support `lea-disp` if it costs nothing (both extraction
  strategies are small). Degrade-never-crash if neither the pattern nor the resolve step lands.
- **Remove the obsolete acquisition:** the current `.interfaces` entry
  `"GameEventManager": "GAMEEVENTSMANAGER002"` and the shim's factory-acquisition block for it (which
  always returns MISSING — the string is in zero modules) are DELETED and replaced by the signature
  path. `s_pGameEventManager` is now populated by the scan, not the factory.

### 2.2 New `.offsets` keys (same file, decimal, dotted keys)

`LoadOffsets(GamedataPath(), "linuxsteamrt64", …)` already backs `GameEntitySystem`. Add these keys
(hex → decimal because JSON has no hex; the spike's HIGH-confidence offsets):

| Key | Hex | **Decimal (stored)** | Meaning |
|---|---|---|---|
| `NetworkServerService.gameServer` | `0x150` | `336` | `*(svc+off)` = `INetworkGameServer*` (GetIGameServer body) |
| `NetworkGameServer.clientCount` | `0x250` | `592` | `int` count of the slot-indexed client vector |
| `NetworkGameServer.clientElems` | `0x258` | `600` | `CServerSideClient**` elems of that vector |
| `ServerSideClient.name` | `0x40` | `64` | `char*` (null → empty) |
| `ServerSideClient.signon` | `0x64` | `100` | `int32` signon state |
| `ServerSideClient.userId` | `0xa8` | `168` | `int16` (`CPlayerUserId`; `−1`/`0xffff` = unassigned) |

Each entry is `{ "linuxsteamrt64": <decimal> }` with the hex in a `//` comment (mirrors the existing
`GameEntitySystem` entry). The engine `m_SteamID`@0xab and the fake/HLTV bool@0xa0 are documented in
the spike but **out of scope** for this slice — steamID is already schema-sourced; HLTV filtering is a
live-soft point handled by the signon check (§7). Keys can be added later without a code change.

---

## 3. Thread A — event delivery (the sig-scan)

### 3.1 The one new engine-generic capability: a pattern scanner (shim)

A self-contained C++ pattern scanner in the shim (engine-generic — no CS2 knowledge):

- **Module bounds:** locate the target module's loaded `.text` (base + size). Use
  `dl_iterate_phdr` (match by soname suffix, e.g. `libserver.so`) to get the load base and the
  executable `PT_LOAD` segment extent; or `dladdr` on a known symbol. The scanner receives
  `(moduleName, .text base, .text size, pattern)`.
- **Match:** linear scan; a pattern byte matches literally, `?` matches anything. Return the address
  of the first match (or null). The spike guarantees uniqueness for the shipped signature; the
  scanner returns first-match and the plan MAY assert-uniqueness in a debug log.
- **Extract (strategy-dispatched):**
  - `lea-disp`: `disp = *(i32*)(match + dispOff)`; `target = match + leaLen + disp`.
  - `ctor-body-xref`: `ctor = match`; scan `.text` for the unique `E8` whose `call` target
    (`callSite + 5 + rel32`) equals `ctor`; from that call site walk back ≤ ~32 bytes to the nearest
    `4C 8D 35`/`4C 8D 2D` (`lea r14/r13,[rip+d]`); `target = leaAddr + 7 + d`.
- The scanner is a pure function of `(bytes, pattern[, xref bytes])` for the match/extract math, so it
  is **unit-testable in-isolate** (synthetic byte buffers), independent of any live process.

### 3.2 Wiring (shim)

Replace the failed `s_pGameEventManager` factory acquisition block (currently tries server then
engine factory → MISSING) with: read the `GameEventManager` signature from
`gamedata/core.gamedata.jsonc` (via `LoadSignatures`) → scan → resolve → cast to
`IGameEventManager2*` (offset-to-top 0, no adjustment) → store in `s_pGameEventManager`. On any
failure (no signatures file, no match, no xref), leave it null and log a named reason — identical to
today's degrade path (event ops become no-ops). Nothing else in the 5D.1 event path changes.

### 3.3 Data flow (events)

```
Load(): scan libserver.so .text (sig from gamedata) → IGameEventManager2* → s_pGameEventManager
        → s_pGameEventManager->AddListener(&s2_listener, name, true)   // per Events.on(name)
runtime: engine fires event → S2ScriptEventListener::FireGameEvent(IGameEvent*) → s_currentEvent = ev
        → s2script_core_dispatch_game_event(name) → event_mux → JS handler(GameEvent accessor)
        → ev.getInt/getString/getPlayerSlot(...)   // block-scoped, synchronous
```

---

## 4. Thread B — engine identity (pure offsets, no scan)

### 4.1 Engine-generic client access (shim + core)

The client list is a Source2 **engine** facility (`libengine2.so`), shared by every Source2 game →
the access is engine-generic. The shim already resolves `INetworkServerService*` (via the engine
factory, `NetworkServerService*V001`). New shim helpers read the list via the §2.2 offsets:

```c
static void* GameServer() {                         // *(svc + gameServer)
    if (!s_pNetworkServerService) return nullptr;
    return *(void**)((char*)s_pNetworkServerService + off_gameServer);
}
static void* ClientAt(int slot) {                   // gs+clientElems[slot], bounds-checked
    void* gs = GameServer(); if (!gs) return nullptr;
    int n = *(int*)((char*)gs + off_clientCount);
    if (slot < 0 || slot >= n) return nullptr;
    void** elems = *(void***)((char*)gs + off_clientElems);
    return elems ? elems[slot] : nullptr;
}
// userId = *(i16*)(c+off_userId); signon = *(i32*)(c+off_signon); name = *(char**)(c+off_name)
```

### 4.2 New engine ops + natives (ABI-appended after the 5D.1 event ops)

Engine-generic, mirroring the entity/event op pattern (Rust `v8host.rs` + C `s2script_core.h`,
ABI-matched, appended so existing indices don't move):

- `__s2_client_valid(slot) → bool` — a client is present at `slot` and (optionally) past a minimum
  signon state (see §7 for the exact predicate; default: non-null client with `signon >=` the
  connected threshold).
- `__s2_client_userid(slot) → i32` — the `int16` user-id widened to i32, or `−1`.
- `__s2_client_signon(slot) → i32` — raw signon state (`−1` if no client).
- `__s2_client_name(slot) → string|null` — a **copied** `v8::String` (the `char*` is read and copied
  in the shim; the raw pointer never crosses to JS), or null.
- `__s2_client_find_by_userid(id) → i32` — first `slot` whose client user-id == `id`, else `−1`.

All are const/by-value; no raw engine pointer crosses into JS. Degrade to `false`/`−1`/`null` on any
null (no service, no game server, empty slot, bad offset). `slot` is validated against the live count.

### 4.3 CS2 player API (games/cs2 — JS + types)

`Player` **is** the CS2 controller (pre-allocated, always constructible via `Player.fromSlot`). The
engine client list is the oracle for "is this slot a connected player + what's their user-id",
independent of pawn presence. In `games/cs2/js/pawn.js` + `packages/cs2/index.d.ts`:

- `player.userId → number` — `__s2_client_userid(this.slot)` (engine, NOT schema; `−1` if unassigned).
- `Player.fromUserId(id) → Player | null` — `__s2_client_find_by_userid(id)` → slot → `fromSlot(slot)`.
- `Player.allConnected() → Player[]` — for `slot` in `0..maxClients`: if `__s2_client_valid(slot)`,
  yield `fromSlot(slot)`. This is the **pawnless** enumeration (connected players regardless of a live
  pawn), complementing the existing pawn-gated `Player.all()`.
- `player.name` / `player.steamID` stay schema-sourced (already proven live in 5B.4). The engine
  `__s2_client_name` is available as the reliable pre-spawn fallback but is not the default `name`
  getter in this slice (YAGNI — add only if the live gate shows schema name is empty pre-spawn).

`Player.all()` (pawn-gated) is UNCHANGED — `allConnected()` is additive, so no regression to the 5C.2
behaviour proven live.

---

## 5. Boundary (the core rule)

| Concern | Lives in | Why |
|---|---|---|
| Pattern scanner, client-list reader, ops, natives | core/shim (engine-generic) | `IGameEventManager2`/`INetworkServerService`/`CServerSideClient` are **Source2 engine** types, not game types — true on any Source2 game |
| The signature + offset VALUES | `gamedata/core.gamedata.jsonc` (the CS2 addon's gamedata) | per-build layout facts (treadmill) |
| `Player.userId`/`fromUserId`/`allConnected` + types | `games/cs2` (JS + `.d.ts`) | `Player` = `CCSPlayerController` (a CS2 class) |

`scripts/check-core-boundary.sh` + `scripts/test-boundary-nameleak.sh` MUST stay green: no CS2
identifier enters `core/src`. (`CServerSideClient` etc. are engine names, not in the CS2 name-leak
pattern set; the plan verifies the gate still passes.)

---

## 6. Degradation & the treadmill

- **Per-fact degrade, never crash globally.** Missing/renamed signature → `s_pGameEventManager` null →
  event ops no-op (today's behaviour). Null service / null game server / out-of-range slot / a wrong
  offset → identity natives return `false`/`−1`/`null`. No path dereferences an unvalidated pointer.
- **Treadmill.** The signature and offsets move per CS2 patch; both are committed gamedata with
  documented derivation anchors (§ spike findings). Regeneration is manual for this slice (like the
  committed `schema-catalog.json`); an auto-regen tool is explicitly out of scope (YAGNI).
- **Live-soft points** (proven only on the gate; each degrades safely if the offline inference is
  wrong): (1) `clientElems[slot]` index == the 0-based `Player`/`Pawn` slot; (2) whether HLTV/proxy
  clients must be skipped; (3) the exact signon-state value(s) denoting "connected / no pawn yet".
  The plan reads signon live across connect to fix the predicate in §4.2.

---

## 7. The live-validation predicate (resolving the soft points)

`__s2_client_valid` and `allConnected` need a concrete "is a real connected player" predicate. The
design: **non-null client AND `signon` at or above the connected threshold**, with the exact
threshold pinned on the gate. The spike saw `cmp signon, 6` (SIGNONSTATE_FULL) and `cmp signon, 1`.
The live gate (T-identity) logs `slot/userid/signon/name` across the bot spawn and `bot_kick`, and the
plan sets the threshold from what it observes (bots reach full signon). If the `0x258` index turns out
NOT to equal the player slot, the fallback is to match the client to a slot via the pawn/controller
(deferred; flagged, not built, if the primary assumption holds — the spike's evidence is strong).

---

## 8. Testing & live gates

- **Unit / in-isolate:** the pattern scanner (literal match, wildcard match, no-match, `lea-disp`
  extraction, `ctor-body-xref` extraction over synthetic buffers) — pure, no live process. The core
  natives follow the existing in-isolate test pattern (null-service degrade paths).
- **One sniper rebuild:** both threads' native changes (scanner+wiring, client ops) land before the
  first live gate, so a single `build-sniper.sh` covers both.
- **Live gate A (events)** — `bot_quota 2`, de_inferno, past the boot window: subscribe to a real
  event (`round_start` and/or `player_connect_full`/`player_spawn`) → observe delivery with correct
  named fields (`getInt`/`getString`/`getPlayerSlot`). This is the first-ever proof of 5D.1 delivery.
- **Live gate B (identity)** — same server: `Player.allConnected()` yields the 2 bots (pawnless-safe),
  `player.userId` reads valid ids, `Player.fromUserId(id)` round-trips to the right slot, `signon`
  logged to pin §7. Both gates degrade to empty on `bot_kick`, server ticking, no crash.

---

## 9. Rough task decomposition (~5; the plan finalizes)

1. **Pattern scanner + event signature + wire `s_pGameEventManager`** (shim, engine-generic) + the
   `.signatures` section & `LoadSignatures` loader in `gamedata/core.gamedata.jsonc` + in-isolate
   scanner tests. Deletes the obsolete `GameEventManager` interface entry + its failed factory block.
2. **Client-list ops/natives + identity offsets** (core `v8host.rs` + `event_mux`-adjacent, shim
   reader, C ABI header) + the 6 `.offsets` keys in `gamedata/core.gamedata.jsonc` + in-isolate
   degrade tests.
3. **[one sniper build] + live gate A (events):** demo subscribes to a real event; observe delivery.
4. **Identity JS + types** (`player.userId`/`Player.fromUserId`/`Player.allConnected()` in `pawn.js`
   + `packages/cs2/index.d.ts`) + a vm-compose test.
5. **Live gate B (identity)** + README/CLAUDE update + spike-findings cross-link.

Threads A and B are separable; if the plan or review surfaces that the slice is too large to land
cleanly, the natural cut is Thread A (events) first, Thread B (identity) as a fast follow — but the
approved scope is both in one branch.

---

## 10. Explicitly out of scope (do not build ahead)

Blocking/pre-hooks/`HookResult` for events; *firing* events; an auto-dumped event catalog; the newer
`IGameEventSystem` protobuf path; engine `m_SteamID` reads (schema already covers it); HLTV/proxy
client typing; a signature/offset AUTO-regeneration tool; the `tsc` typecheck gate; config/permissions;
the registry/platform (5.5); the base-plugin suite (6). Note later needs as TODOs and stop.
