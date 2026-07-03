# Slice 5D.1b — engine-RE feasibility spike findings (game-event-manager signature)

**Question the spike answered:** can we obtain the un-exported process-global `IGameEventManager2*`
in CS2 — the pointer 5D.1 needs but that `CreateInterface("GAMEEVENTSMANAGER002")` cannot return —
by signature-scanning the on-disk game module in THIS environment? (Flagged all session as
feasibility-risky; the instruction was to spike hard and report BLOCKED honestly if it couldn't be
cracked here.)

**Verdict: RECIPE-FOUND (high confidence).** Not blocked. The signature is derivable offline from the
pinned live-gate binaries; the only residual unknown (is the instance live + does `AddListener`
deliver) is precisely what the existing live gate validates.

## Environment inputs (why it's feasible here)

- The **pinned** CS2 binaries are on disk in `docker/cs2-data/game/…` (BuildID
  `d413735360ecfd8eefd3b5d5d87b1959a39dd9ef`), so a signature derived offline is valid for the live
  gate. `libserver.so` is 39 MB and **stripped**.
- Disassembly tooling (`objdump`/`readelf`/`nm`/`strings`) is present; a runtime pattern scan in the
  shim is trivial C++ (module base+size via `dl_iterate_phdr`/`dladdr`).
- No CounterStrikeSharp install here (the stray `counterstrikesharp.log` is a leftover) → **no
  checked-in community signature to crib**; the signature below was derived from first principles
  (RTTI → vtable → xref).

## Binary-level confirmation of the 5D.1 finding

- `GAMEEVENTSMANAGER002` appears in **zero** modules → the legacy manager is genuinely not a
  registered interface (deepens 5D.1's live "MISSING" to a binary fact). The RTTI (`17CGameEventManager`,
  `18IGameEventManager2`) IS present → the class exists; only the global instance is reachable.
- `IGameEventSystem` (the newer networked bus) IS factory-acquirable (`GameEventSystemServerV001`),
  but `PostEventAbstract` hands a serialized `CNetMessage` (protobuf), not a named-field `IGameEvent*`
  — a much larger mechanism that would still need the legacy manager to unserialize. So the
  **least-mechanism path is the single sig-scan for the legacy manager**, reusing all of 5D.1 verbatim.

## The recipe (module = `libserver.so`, scan `.text`)

- **Instance:** `IGameEventManager2*` singleton at RVA `0x26e0860` (`.bss`, writable NOBITS — correct
  for a runtime-constructed vptr object). Offset-to-top 0 ⇒ no pointer adjustment.
- **Global pointer** `g_pGameEventManager` @ RVA `0x2363320` (`.data.rel.ro`) holds `&instance`; a
  runtime virtual-call site (`0xe631e6`: `lea …[0x2363320]; mov rbx,[rax]; mov rax,[rbx]; call [rax+0x38]`)
  proves it's the live polymorphic instance.

**Primary signature** (constructor call-site; unique — 1 match in `.text`, at `0x15b18aa`):

```
4C 8D 35 ?? ?? ?? ?? E8 ?? ?? ?? ?? 4C 89 F7 E8 ?? ?? ?? ??
lea r14,[rip+disp32] ; call helper ; mov rdi,r14 ; call CGameEventManager::ctor
```
Extraction: `disp = *(i32*)(match+3)`; `lea` is 7 bytes ⇒ `instance = match + 7 + disp` (RIP-relative,
yields the runtime VA directly).

**Fallback signature** (ctor BODY — recommended primary for the gamedata generator; more update-stable
because a function body survives register-allocation churn better than one call site; unique — 1 match,
ctor at `0x1590400`):

```
55 48 8D 05 ?? ?? ?? ?? BE 40 00 00 00 48 89 E5 41 54 4C 8D 65 E8 53 48 89 FB
push rbp ; lea rax,[rip→vtable 0x2361350] ; mov esi,0x40 ; mov rbp,rsp ; push r12 ;
lea r12,[rbp-0x18] ; push rbx ; mov rbx,rdi
```
Runtime step: find the single `E8` rel32 whose target is the ctor; from that call site walk back ≤~32
bytes to the nearest `4C 8D 35`/`4C 8D 2D` (`lea r14/r13,[rip+d]`); `instance = lea_addr + 7 + d`.
Validated: sole caller `0x15b18b9` → preceding `lea 0x15b18aa` → `0x26e0860`.

## Shim runtime pseudocode

```c
u8* m = sig_scan(libserver_text, PATTERN);          // wildcard '?' match over .text
if (!m) { /* degrade: manager stays null, event ops no-op, named reason logged */ }
i32 disp = *(i32*)(m + 3);
IGameEventManager2* mgr = (IGameEventManager2*)(m + 7 + disp);   // == 0x26e0860 on the pinned build
// existing 5D.1 path: mgr->AddListener(&s2_listener, name, true); … FireGameEvent(IGameEvent*) delivers
```

## Honesty caveats

- Byte signatures are patch-fragile — **this IS the treadmill**: the signature is a regenerable
  per-update gamedata artifact, not code. The ctor-body fallback is the more robust primary.
- Offline analysis cannot prove the instance is fully constructed/registered at shim-run time, nor
  that this legacy manager (vs. `CCustomGameEventManager`) carries the events the base plugins want.
  The 5D.1 mechanism already handles "non-null → deliver", so the final proof is: drop the pointer in,
  subscribe on the live gate, watch for delivery.
- Degrade-never-crash: a missing/wrong signature disables just the event capability with a named
  reason (identical to today's no-op behaviour), never a global crash.

## Reproducible (decisive commands)

```bash
cd docker/cs2-data/game/csgo/bin/linuxsteamrt64
strings -a -t x libserver.so | grep -E '17CGameEventManager|18IGameEventManager2'   # RTTI name strings
objdump -d --start-address=0x1590400 --stop-address=0x1590428 -M intel libserver.so # ctor: lea vtable; mov [rdi],rax
objdump -d --start-address=0x15b18aa --stop-address=0x15b18be -M intel libserver.so # lea r14,#26e0860; mov rdi,r14; call ctor
objdump -d --start-address=0xe631e6  --stop-address=0xe63204  -M intel libserver.so # virtual call through *(0x2363320)
```

---

# Engine-identity spike findings (connected-client list → userId / pawnless enum)

**Verdict: PARTIAL — usable core fully cracked** (no target BLOCKED; a few points need live
confirmation). All values are `libengine2.so` load-relative VAs on the pinned build; they are
**offsets/indices = gamedata** (regenerable per patch via the documented anchors), never hardcoded in
code. Engine identity needs **NO signature scan** — every value is a fixed member offset off pointers
we already hold. (Contrast: the event manager needs the sig-scan because it's a global with no
interface.)

## Target 1 — reach the game server — HIGH
- `INetworkServerService::GetIGameServer` = **vtable slot 24**, body `mov rax,[rdi+0x150]; ret`.
- **Recommended:** read the member directly — `gameServer = *(void**)((char*)svc + 0x150)` — more
  update-robust than a vtable index. `svc` = `NetworkServerService*V001` (already resolved in-project).

## Target 2 — the client vector inside `CNetworkGameServer` — HIGH
- Slot-indexed list: **count @ `gs+0x250`, elems @ `gs+0x258`** (`CUtlVector<CServerSideClient*>`);
  kick-by-slot and the status builder both index `gs+0x258[slot]`, so **index == player slot**.
- Alternate dense list @ `gs+0x80`/`gs+0x88`. Use `gs+0x258` for slot→client and enumeration.

## Target 3 — `CServerSideClient` field offsets — HIGH (object size 0xf70)
| Field | Offset | Type | Anchor |
|---|---|---|---|
| m_Name | 0x40 | `char*` (null→empty) | name `%s` in kick/status/logaddress; ctor null-inits |
| m_nSignonState | 0x64 | int32 | `cmp [c+0x64],1` / `,6` (SIGNONSTATE_FULL); ctor `=0` |
| m_UserID | 0xa8 | int16 (`CPlayerUserId`) | Connect stores `WORD [c+0xa8]`; ctor default `0xffff` (−1) |
| m_SteamID | 0xab | uint64 (**unaligned**, `#pragma pack(1)` block 0xa8–0xb2) | steamid render getter; Connect |
| fake/HLTV | 0xa0 | bool | type-guard across kick/getter/status |

## Shim walk
```c
void* svc = engineFactory("NetworkServerService001");
void* gs  = *(void**)((char*)svc + 0x150);   if (!gs) return;   // no active server → degrade
int    n     = *(int*)   ((char*)gs + 0x250);
void** elems = *(void***)((char*)gs + 0x258);
for (int slot = 0; slot < n; slot++) {
    void* c = elems[slot]; if (!c) continue;                    // empty slot
    int      signon = *(int*)     ((char*)c + 0x64);
    int16_t  userid = *(int16_t*) ((char*)c + 0xa8);            // −1 if unassigned
    const char* name = *(const char**)((char*)c + 0x40);       // may be NULL
}
```

## Needs live validation (design must degrade gracefully on each)
1. `gs+0x258` index == the 0-based slot the existing `Player`/`Pawn` system uses (strongly implied).
2. Which vector is canonical + whether to skip HLTV/proxy clients (filter via the 0xa0 bool).
3. Exact signon-state enum values for the connecting / no-pawn states (read 0x64 live across connect).

## Anchors for regeneration (the treadmill)
`%s<%i><%s><>`@0x261a39 (logaddress) · `"%s" disconnected`@0x25f583→fn 0x5d1450 (0x80/0x88) ·
`%s kicked by %s (%s)`@0x25f272→fn 0x698840 (0x250/0x258 + fields) · steamid getter 0x6bbb20 (0xab) ·
Connect 0x6b8c90 (userid WORD @0xa8) · ctor/factory 0x6a6ab0 (defaults).
