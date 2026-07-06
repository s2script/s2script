# Sub-project 3 — the ban-reason flip (retire the instant-reject; enforce in JS with a visible reason)

**Goal:** Wire the ban-reason feature end-to-end. Retire the 6.18 shim `ClientConnect` instant-reject and make `@s2script/basebans` enforce bans in JS via `Clients.onConnect → kickWithReason`, so a banned player is **admitted → shown their reason (chat + console) → kicked**. This is the payoff of the whole thread and the one **behavior change** in it.

**Non-goals / deferred:**
- **Full live verification needs a human client** (the admit→show-reason→kick + the reconnect flow can't be exercised by bots — `ClientConnect`/`OnClientConnected` for a *banned real player* is the whole point). Built + merged with the live check deferred (the user runs it when next connected), same pattern as sub-projects 1–2.
- **The slot-reuse robustness follow-up** (a userId re-check at the delayed-kick fire time) stays deferred — the map-load time (>5s) already makes the same-slot-reconnect-within-delay window effectively unreachable.
- **A 3rd-party async ban provider** is *enabled* by this design (they write their own `Clients.onConnect` handler querying their DB → `c.kickWithReason(...)`), but we only ship our `BAN_CACHE`-backed default here.

## What changes

### 1. Shim — retire the `ClientConnect` instant-reject (flip to always-admit)
The 6.18 `ClientConnect` SourceHook (which called `s2script_core_ban_check` and returned `MRES_SUPERCEDE,false` to instant-reject a banned SteamID) is **removed entirely**. With no reject, `OnClientConnected` (sub-project 1's hook) now fires for *every* connecting client — including banned ones — driving the JS `onConnect` event that enforces the ban. Remove:
- The `SH_DECL_HOOK6(... ClientConnect ...)` decl (`s2script_mm.cpp:76-79`).
- The `Hook_ClientConnect` member (decl in `s2script_mm.h` + the body — the `ban_check`+`RETURN_META_VALUE(MRES_SUPERCEDE,false)` function).
- The `SH_ADD_HOOK(... ClientConnect ...)` + its `"ClientConnect hook installed (ban enforcement)"` log (`:1203-1206`).
- The `SH_REMOVE_HOOK(... ClientConnect ...)` (`:1633-1636`).
- The `#include <ctime>` (`:53`) IF `time()` is now unused elsewhere in the shim (grep — it was added for this hook; remove if orphaned).
- **Keep** `s2script_core_ban_check` (the core ffi export + its `s2script_core.h` decl), `BAN_CACHE`, and the `__s2_ban_*` natives — the store is unchanged; `ban_check` is retained as an available *synchronous* ban-check primitive (now unused by core, documented as such — a 3rd party could still call it). This keeps the change minimal and the store intact.

**Result:** the shim no longer rejects anyone; the JS layer owns ban enforcement.

### 2. `@s2script/basebans` — the JS enforcement (the reference ban provider)
In `onLoad`, register the connect-time enforcement (engine-generic logic, living in the reference plugin so 3rd parties can write their own alongside/instead):
```ts
import { Clients } from "@s2script/clients";
// ... in onLoad:
Clients.onConnect((c) => {
  if (c.isBot) return;                                  // bots have steamId "0" — never banned
  const b = Bans.get(c.steamId);
  if (!b) return;
  const now = Date.now() / 1000;
  if (b.until !== 0 && b.until <= now) return;          // expired — let them in
  const expiry = b.until === 0 ? "permanent" : "expires in " + Math.ceil((b.until - now) / 60) + " min";
  c.kickWithReason("[SM] You are banned: " + (b.reason || "No reason") + " (" + expiry + ")");
});
```
- `kickWithReason` (sub-project 2) sets the pending entry now (at connect) and delivers + kicks when the client reaches `onActive` — the admit→show-reason→kick flow.
- The expiry check mirrors the retired shim `ban_check` logic (`until === 0` permanent; `until > now` active) — an expired ban no longer blocks (correct).
- `sm_ban` still kicks the *online* player immediately (`player.kick`, unchanged); this handler is the *reconnect* enforcement.

### 3. Unchanged
`@s2script/bans` (the store: `BAN_CACHE`/`Bans.add/get/remove/list/reload`/`bans.json`), the `sm_ban`/`sm_unban`/`sm_addban` commands, sub-projects 1–2, and everything else.

## Testing

- **Typecheck:** `@s2script/basebans` full-strict against the shipped `.d.ts` (it now imports `@s2script/clients` for `Clients`/`Client.kickWithReason`).
- **Boundary:** the shim change is a removal (engine-generic core untouched); `@s2script/basebans` stays the only CS2 piece. Both gates green.
- **In-isolate:** no new core logic (the shim removal has no unit surface; the basebans handler is exercised live). The existing 153/0 core tests stay green (nothing core changed except the retained-but-unused `ban_check`).
- **Live gate (bots-provable now):** the deploy loads; `basebans onLoad` registers the `onConnect` enforcement; a bot connecting is NOT kicked (steamId "0" → `Bans.get` null); the `"ClientConnect hook installed"` log is GONE (reject retired) while the six lifecycle hooks + `sm_ban`/`sm_unban` still register; sub-project-1/2 unaffected; `RestartCount=0`, no crash.
- **Live gate (deferred, human client — the payoff):** `sm_ban <self> 1 test` → kicked immediately (unchanged); **reconnect → now ADMITTED (loads in), then chat + console show `[SM] You are banned: test (expires in N min)`, then kicked ~5s later** (NOT an instant `NETWORK_DISCONNECT_REJECTED_BY_GAME` like 6.18); `sm_unban` → reconnect clean; an expired ban lets them in. This is the behavior change to verify with the user.

## Risks / decisions

- **Behavior change (the one real risk):** bans go from instant-reject to admit→show→kick. A banned player now briefly occupies a slot + loads the map before the kick — the accepted tradeoff for showing the reason. If the JS enforcement fails to fire, a banned player would stay in (degraded bans) — the live gate with a human is the definitive check; the bots-provable gate confirms the handler registers + doesn't crash + doesn't kick innocents.
- **Retained `ban_check`:** kept (unused) rather than removed to keep the change minimal + the store/primitive intact. Documented as available-synchronous.
- **Atomic flip:** the shim removal + the basebans enforcement must deploy TOGETHER (removing the reject without the JS enforcement = bans don't enforce at all). One slice, one deploy, one live gate.

## Build order

- **Task 1 (shim + basebans, the atomic flip):** remove the `ClientConnect` reject hook (shim) AND add the `Clients.onConnect` enforcement (basebans) in one task — they are coupled (deploy together). Sniper rebuild (shim changed). Typecheck basebans.
- Then: deploy + bots-provable live gate + merge (human-client verification deferred).
