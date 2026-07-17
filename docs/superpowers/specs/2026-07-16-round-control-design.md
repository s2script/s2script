# Round control — design spec

**Date:** 2026-07-16
**Status:** design (autonomous) → implementation plan `docs/superpowers/plans/2026-07-16-round-control.md`
**Scope:** ONE new engine fact (`CCSGameRules::TerminateRound`) + a pure-reuse CS2-package surface (round clock write, team scores, `RoundEndReason`). Worktree: `s2script-round-control`.

## 1. Problem & consumer

The TTT port (and any round-orchestrating gamemode: warmup managers, comp wrappers, minigames) needs to **drive the CS2 round lifecycle**, not just observe it. TTT's concrete surface (`TTT/CS2/Utils/RoundUtil.cs`, `RoundTimerListener.cs`) is exactly seven operations:

| TTT op | s2script status |
|---|---|
| `EndRound(reason)` → `CCSGameRules::TerminateRound` | **THE gap** — no call mechanism exists anywhere (grep of core/shim/gamedata: zero hits) |
| `mp_ignore_round_win_conditions` toggling | EXISTS — `Server.command` (queued next-frame; sufficient, see §5) |
| Synthetic `cs_win_panel_round` (`final_event`) / `nextlevel_changed` | EXISTS verbatim — `Events.fire(name, fields, dontBroadcast)`; both in the 272-event catalog |
| `Get/SetTimeRemaining`, `GetTimeElapsed` (round clock) | primitives EXIST (`writeInt32Via` + `notifyStateChanged`); missing only the typed surface + the `m_fRoundStartTime` getter |
| `IsWarmup` | EXISTS — `GameRules.get().warmupPeriod` |
| `Get/Set/AddTeamScore` (`cs_team_manager` / `CTeam.m_iScore`) | primitives EXIST (`findByClass`, `readUInt8`, `writeInt32`, `notifyStateChanged`); missing only the typed surface |
| `Events.on("round_end")` with self-caused filtering | EXISTS — catalog + plugin state |

**Slice shrink (decision):** the only genuinely NEW engine facts are (1) the `TerminateRound` signature + call op and (2) the `RoundEndReason` numeric map (reviewed code, binary-validated). Everything else is game-package JS over existing, live-proven ops. No new read/write primitive, no new event infra, no cvar machinery.

## 2. The engine facts

### 2.1 `CCSGameRules::TerminateRound` — self-resolve + semantic load validation

**The doctrine's worst-case failure mode is ACTIVE here, not theoretical.** The CSSharp/Swiftly Linux byte-sig (`55 48 89 E5 41 57 41 56 49 89 FE 41 55 41 54 53 48 81 EC ? ? ? ? 48 8D 05 ? ? ? ? F3 0F 11 85`) scans to **exactly 1 hit** on our pinned build 2000875 — and it is the **wrong function** (va `0xc75ec0`: treats `esi` as a *pointer*, no reason bound-check, no SFUI switch). A plain uniqueness gate would green-light a corrupting call. Re-verified offline in this worktree (2026-07-16): borrowed → 1 hit @ `0xc75ec0`; fresh → 1 hit @ `0x1384e80`.

**The real function** (va `0x1384e80` on 2000875) is anchored by three independent facts:
1. a unique `lea rsi,[rip+disp]` to the literal C-string `"TerminateRound"` (telemetry scope name, va `0x8edd6c`, exactly 1 xref binary-wide) at **fn+0xb**;
2. the sole xref to `"TerminateRound: unknown round end ID %i"` lands in this function's own cold branch, reached by `cmp $0x16,%r15d; ja …` — binary-validating the reason enum bound (22 = `SurvivalDraw`);
3. the CS:GO-inherited reason→`#SFUI_Notice_*` switch table in the body, 16 direct callers in the gamerules TU.

**Ship the SELF-DERIVED prologue pattern** (masks the volatile string disp + frame size, keeps the stable `41 89 F7` reason capture — the ChangeTeam masking style):

```
55 48 89 E5 41 57 41 89 F7 41 56 48 8D 35 ? ? ? ? 41 55 41 54 53 48 89 FB 48 81 EC ? ? ? ?
```

**Prototype (Linux SysV, proven by register flow at `0x1384e80`):**

```c
void CCSGameRules::TerminateRound(float delay /*xmm0*/, uint32 reason /*esi*/,
                                  void* unk3 = 0 /*rdx*/, uint32 unk4 = 0 /*ecx*/);
```

TTT/CSSharp's "reason-first" Linux invoker order is a managed-marshaller artifact (int-class args listed in int-reg order with the float pulled out to xmm0) — both decls hit the same registers. **The direct C call must be delay-first**; copying the C# order would swap `delay` into the reason register. `unk3`/`unk4` are unknown in every framework (all pass 0,0) — hardcoded to 0, never exposed. **Not virtual on this build** (zero data-segment qword refs) — direct call only.

**Load validation — uniqueness is NOT enough (new for this descriptor):** after `ResolveSigValidated` passes, a **semantic check** verifies the masked lea at fn+0xb really targets the C-string `"TerminateRound"`: reuse `s2sig::ResolveLeaDisp(text, size, fnOff + 0xb, 3, 7)` (the pattern pins the `48 8D 35` opcode bytes, so the lea's presence is already guaranteed), follow the target, `memcmp`. The string lives in the R-only LOAD segment *below* the PF_X base (verified: target offset is negative relative to `.text`), so the check range-guards against the module's **full mapped extent** via a new `FindModuleBounds` helper (a `dl_iterate_phdr` sibling of `FindModuleText`, keyed to the same largest-PF_X module). Result is reported as its own boot-gate descriptor line `TerminateRound.scope-string` — on failure, `s_pTerminateRound` stays null with the named reason *"prologue lea does not reference the 'TerminateRound' scope string (unique-but-WRONG match — the borrowed-sig trap)"*. This is precisely the check that distinguishes the real function from the false positive the borrowed sig hits.

**Treadmill recipe (documented in the gamedata comment):** the masked pattern survives disp/frame-size drift; if register allocation changes, re-resolve by xref'ing the two unique strings (`"TerminateRound"` single-lea; `"TerminateRound: unknown round end ID %i"` single-xref).

**Reason bound:** the shim rejects `reason < 0 || reason > 22` (host-side mirror of the engine's `cmp $0x16` check). In-range legacy holes (2/3/15) pass through — the engine's own switch handles them; rejecting them would hardcode game semantics we don't own.

### 2.2 `RoundEndReason` + `cs_win_panel_round` `final_event` — reviewed code, validated numbers

Per **"layout is data, semantics are code"**: these are name↔number mappings, not offsets → they ship as a reviewed `const` map in the CS2 package. Sources are HINTs (CSSharp enum; TTT's `final_event` 2/3), but the reason values are **already binary-validated**: the `cmp $0x16` bound (max 22) + every `#SFUI_Notice_*` string present in the body switch. The `CSRoundEndReason` enumerators are absent from our schema dump, so codegen can't carry them. Residual live validation is **free and closed-loop**: the demo passes `reason` to `terminateRound` and compares it against the engine-emitted `round_end.reason`; `final_event` is checked against a natural round end's `cs_win_panel_round`.

### 2.3 Everything else — existing primitives, zero new engine facts

- **Round clock write** = `proxyRef.writeInt32Via([off(CCSGameRulesProxy,m_pGameRules)], off(CCSGameRules,m_iRoundTime), v)` + `proxyRef.notifyStateChanged(off(CCSGameRulesProxy,m_pGameRules))` — the exact TTT `SetStateChanged(proxy,"CCSGameRulesProxy","m_pGameRules")` semantics. The notify is a FLAT offset on the PROXY root; no chain-notify variant needed. Live-proven primitive, but proxy-notify-renetworks-the-pointed-to-struct is a borrowed *pattern* → the HUD clock visibly updating is a mandatory live-gate criterion (§7).
- **`roundStartTime`** = one `grFloat("m_fRoundStartTime")` getter (GameTime_t read as f32; validated live: ≈ `Server.gameTime` at `round_start`).
- **Team scores** = `Entity.findByClass("cs_team_manager")` → match `m_iTeamNum` (never assume ordering — ~4 team entities) → `writeInt32(off(CTeam,m_iScore))` + `notifyStateChanged(same offset)`. Re-find per call (cold path) — deliberately NO cache; TTT's `_teamManager ??=` cache is a map-change bug we don't replicate.
- **Synthetic events** = `Events.fire("cs_win_panel_round", {final_event}, false)` / `Events.fire("nextlevel_changed", {}, false)` — already exact TTT parity (`CreateEvent(name, bForce=true)`).

## 3. API shape (`@s2script/cs2` — packages/cs2/index.d.ts + games/cs2/js/pawn.js)

PascalCase types/consts, camelCase methods (Slice-4 convention). All additions are **additive** (existing `readonly` fields untouched → 5E.1-safe). Writers are **methods returning `boolean`** — not property setters — so a failed write (stale proxy, unresolved offset) is detectable, never silently assumed (a research-flagged risk).

```ts
/** CS2 round-end reasons (CCSGameRules::TerminateRound / round_end.reason). Values binary-validated
 *  against our build's bound check (max 22) + the in-body #SFUI_Notice_* switch; gaps 2/3/15 are
 *  removed legacy VIP reasons. */
export declare const RoundEndReason: {
  readonly Unknown: 0; readonly TargetBombed: 1; readonly TerroristsEscaped: 4;
  readonly CTsPreventEscape: 5; readonly EscapingTerroristsNeutralized: 6; readonly BombDefused: 7;
  readonly CTsWin: 8; readonly TerroristsWin: 9; readonly RoundDraw: 10;
  readonly AllHostagesRescued: 11; readonly TargetSaved: 12; readonly HostagesNotRescued: 13;
  readonly TerroristsNotEscaped: 14; readonly GameCommencing: 16; readonly TerroristsSurrender: 17;
  readonly CTsSurrender: 18; readonly TerroristsPlanted: 19; readonly CTsReachedHostage: 20;
  readonly SurvivalWin: 21; readonly SurvivalDraw: 22;
};

/** cs_win_panel_round final_event values (HINT from TTT/CSSharp usage; validated at the live gate
 *  against a natural round end). */
export declare const WinPanelFinalEvent: { readonly CTsWin: 2; readonly TerroristsWin: 3 };

export interface GameRulesView {
  // …existing 12 readonly fields unchanged…
  /** m_fRoundStartTime (GameTime_t): curtime when the current round started. */
  readonly roundStartTime: number | null;
  /** Server.gameTime - roundStartTime - freezeTime (TTT GetTimeElapsed). */
  readonly timeElapsed: number | null;
  /** roundTime - timeElapsed (TTT GetTimeRemaining). */
  readonly timeRemaining: number | null;
  /** Write m_iRoundTime + renetwork (proxy notifyStateChanged at m_pGameRules). false = write failed. */
  setRoundTime(seconds: number): boolean;
  /** Set the remaining round time (roundTime = elapsed + seconds). */
  setTimeRemaining(seconds: number): boolean;
  /** Extend/shrink the round clock by delta seconds. */
  addTimeRemaining(seconds: number): boolean;
  /** Force the round to end with a RoundEndReason. QUEUED: executes on the NEXT engine frame
   *  (outside the JS isolate borrow) so every plugin's round_end handler — including the caller's —
   *  fires normally. true = queued; false = degraded (unresolved sig / stale proxy / bad reason). */
  terminateRound(reason: number, delay?: number): boolean;
}

export declare const GameRules: {
  get(): GameRulesView | null;
  /** Convenience over get()?.terminateRound(). delay defaults to 5s (TTT parity). */
  terminateRound(reason: number, delay?: number): boolean;
};

/** cs_team_manager score access (CTeam.m_iScore + notifyStateChanged). team = 0..3
 *  (Unassigned/Spectator/T/CT); entities matched by m_iTeamNum, re-found per call. */
export declare const Teams: {
  getScore(team: number): number | null;
  setScore(team: number, score: number): boolean;
  addScore(team: number, delta: number): boolean;
};
```

**Why this shape:** `terminateRound` lives on the view (it owns the serial-gated proxy ref) with a static convenience on `GameRules` because "end the round" is the headline verb and forcing `get()` boilerplate on every caller is hostile; `Teams` is its own const (scores are team-entity state, not gamerules state); reason is `number` typed against the `RoundEndReason` const values (no TS enum — consts are the locked ChatColors precedent). The queued/one-frame-latency semantics are **documented in the .d.ts** because they are observable (a terminate followed by an immediate state read sees the old round).

## 4. Architecture & dispatch

### 4.1 One new op, deferred execution (the re-entrancy decision)

`TerminateRound` fires the round-end machinery (round_end event) **synchronously inside the call**. Core holds `HOST.borrow_mut()` across all JS, so an inline call from any JS context (command handler, timer, event handler — exactly TTT's flows) would re-enter dispatch, hit the `try_borrow_mut` graceful-skip, and **every plugin would silently miss round_end** for that round. This is the slice's most likely silent failure, so deferral is baked into the op, not left to callers:

- `s2_gamerules_terminate_round(idx, serial, rules_ptr_off, delay, reason) -> int` **enqueues** a single-slot pending request (latest-wins, logged on overwrite — a round ends once) and returns 1; 0 on any degrade (unresolved sig, out-of-range reason, stale proxy at enqueue).
- A dedicated `Hook_GameFrameRoundDrain` SourceHook pre-hook on `ISource2Server::GameFrame` — installed **eagerly at Load iff the sig resolved** (a per-frame `if (!armed) return` is negligible; eager install avoids mutating the hook chain from inside a frame dispatch) and removed at Unload — drains the slot **outside the JS borrow**: re-derefs the proxy handle (serial-gated at drain time, not just enqueue time), follows `*(void**)(proxy + rules_ptr_off)` to the rules struct, `.text`-range-guards the fn pointer (the ChangeTeam guard), and calls `s_pTerminateRound(rules, delay, reason, nullptr, 0)`. The resulting synchronous round_end flows through the normal FireEvent pre-hook → core dispatch → all JS subscribers.
- Cost of the decision: **one frame of latency** (~15ms), documented in the .d.ts. Against it: TerminateRound's own `delay` parameter already makes sub-frame timing meaningless.

**Rules-pointer path:** JS passes the proxy `(index, serial)` + the `m_pGameRules` offset (resolved in pawn.js via `__s2_schema_offset` — the game package owns the class/field names); the shim derefs. No raw pointer ever crosses to JS; no CS2 name crosses the C ABI (the `player_change_team` precedent, extended with one offset arg — the same shape `writeInt32Via` uses).

### 4.2 ABI append point

Append **exactly one** op after `usercmd_clear_subtick` (the confirmed byte-identical tail in `core/src/v8host.rs` struct @241 and `shim/include/s2script_core.h` struct closing @376), in all five touchpoints (C typedef+field, Rust typedef+field, BOTH in-test literals, shim `ops.` wiring):

```c
typedef int (*s2_gamerules_terminate_round_fn)(int idx, int serial, int rules_ptr_off,
                                               float delay, int reason);
```

**Coordination flag:** the unmerged `feat/writeconvar` branch pins its op append after a stale tail — whichever of these two merges second must re-tail and update both test literals in the same commit.

### 4.3 The cvar question (decision: queued `Server.command` SUFFICES)

TTT's `mp_ignore_round_win_conditions 1 → TerminateRound → 0` bracket is **ordering-illusory even in CSSharp**: `Server.ExecuteCommand` is buffered `ServerCommand`, so TTT's direct call always executes *before* both buffered sets — it only works because TTT holds the cvar at 1 from COUNTDOWN start for the whole round. s2script adopts the same **hold-across-the-round** pattern with the existing queued `Server.command`/`setCvar`; our one-frame terminate deferral even lands *after* a same-frame queued set, strictly better than CSSharp's ordering. Therefore: **no synchronous cvar write in this slice** (it stays `feat/writeconvar`'s scope; a previously-deferred injection surface). Consumers must: set the cvar at round start, re-assert on `Server.onMapStart` (map change resets it), never interpolate user input into `setCvar`, and document operator recovery (`mp_ignore_round_win_conditions 0`) for a crash-while-held.

### 4.4 Plan B — `TerminateRound` unresolvable (the fallback IS this slice's other half)

If either gate (uniqueness or scope-string) fails on a future build: the op degrades (returns 0, named boot reason), the framework keeps running, and the documented fallback is **timer-expiry forcing** using this same slice's write surface — ensure `mp_ignore_round_win_conditions 0` (queued, one frame earlier), then `setTimeRemaining(0)`; the engine's own `CheckWinConditions` ends the round on timeout. Accepted degradations: the engine picks the timeout reason (winner attribution may be wrong for custom-gamemode wins — compensate the UI with a synthetic `cs_win_panel_round` + manual `Teams.setScore`), and timing quantizes to the win-condition check cadence. `mp_restartgame` is NOT an acceptable fallback (resets match scores).

## 5. Boundary check (core vs @s2script/cs2) — litmus per piece

| Piece | Home | Litmus: true on another Source 2 game? |
|---|---|---|
| Op slot + `__s2_gamerules_terminate_round` native | core | Yes as *plumbing*: "deref a serial-gated entity, follow one pointer-field offset, invoke a gamedata-resolved fn" carries zero CS2 names across the ABI (the `player_change_team` precedent). Core never knows what the fn does. |
| `TerminateRound` sig, prototype, drain queue, scope-string check | shim + gamedata | No (CS2 function) → shim owns it; the sig is regenerable data. `FindModuleBounds`/lea-follow are engine-generic helpers. |
| `"CCSGameRulesProxy"`/`"CCSGameRules"`/`"CTeam"`/`"cs_team_manager"` strings, `GameRules`/`Teams`/`RoundEndReason`/`WinPanelFinalEvent` | games/cs2 + packages/cs2 | No (CS2 classes/protocol values) → game package only. |
| Round clock/team-score write mechanics | already-shipped core primitives | Yes (generic entity writes) — nothing new. |

Gates: `make check-boundary`, `./scripts/test-boundary-nameleak.sh` (no CS2 name enters `core/src`), `./scripts/check-plugins-typecheck.sh` (additive .d.ts).

## 6. Deferred / out of scope (do NOT build ahead — log in docs/PROGRESS.md)

- **Player respawn** (`RoundTimerListener.cs` respawn loops) — fresh RE (its own sig), NOT needed for a first working round (TerminateRound's engine restart respawns everyone). The one hidden TTT dependency; named so it isn't discovered mid-port.
- **Synchronous cvar write** — `feat/writeconvar`'s slice (§4.3).
- **`CCSTeam` codegen accessors** — the curated `Teams` API covers the consumer; adding the class to `codegen-classes.json` is mechanical breadth that bloats the pawn.js concat with no current caller.
- **freezeTime/warmupPeriod setters, SetClan/render-color cosmetics, warmup manipulation, a generic round-state-machine abstraction** — TTT owns its own state machine; s2script ships events + reads + writes + terminate.
- **`mp_ignore_round_win_conditions` sugar on the op** (`{ignoreWinConditions}`) — revisit only if the hold-pattern proves insufficient live.

## 7. Live-gate plan (Docker CS2, `scripts/rcon.py`; MAXPLAYERS=2)

Demo `examples/round-control-demo` (commands `sm_endround`, `sm_settime`, `sm_addtime`, `sm_teamscore`, `sm_winpanel`; loggers on `round_start`/`round_end`/`cs_win_panel_round`).

1. **Boot markers:** `gamedata OK TerminateRound` AND `gamedata OK TerminateRound.scope-string`; `GAMEDATA VALIDATION: N ok, 0 FAILED`.
2. **Terminate + re-entrancy + closed-loop enum (bot-provable):** `sm_endround 9` from a command handler (a JS dispatch — the exact hazard path) → round visibly ends; the demo's own `round_end` logger fires (deferred-drain proof) with `reason=9` (enum read-back proof). Repeat with `8` (CTsWin).
3. **Natural round end:** with `mp_ignore_round_win_conditions 0` let the round timer expire → log engine-emitted `round_end.reason` + `cs_win_panel_round.final_event` → validate the shipped consts.
4. **Round clock (the borrowed-pattern check):** `sm_settime 30` → `roundTime` read-back changes AND the round actually ends ~30s later; HUD clock repaint is the human-visual criterion (append to the deferred-live-tests memory if no human joins).
5. **Team scores:** `sm_teamscore 2 15` → read-back 15; scoreboard visual = human criterion.
6. **roundStartTime sanity:** `round_start` log shows `roundStartTime ≈ Server.gameTime` and `timeElapsed ≈ 0`.
7. **No-crash soak:** several terminate cycles back-to-back (latest-wins overwrite path) + a map change (proxy cache self-heal).

## 8. Risks / STOP conditions

- **STOP if the scope-string gate fails at boot** on the pinned build — the sig landed on the wrong function; do NOT weaken the check to ship. Re-resolve via the string-xref recipe (§2.1).
- **STOP if step 2 of the live gate ends the round but the demo's round_end logger does NOT fire** — the deferral isn't actually outside the borrow; fix the drain, don't ship one-plugin-visible termination.
- **STOP if the HUD clock never repaints** after `setRoundTime` read-back succeeds — the proxy-notify pattern doesn't renetwork on CS2; investigate chunk-level dirty marking before exposing `setTimeRemaining` as working.
- `unk3`/`unk4` semantics unknown — a future build using them changes behavior silently; treadmill note lives in the gamedata comment.
- Windows port footgun: the true prototype is `(float, uint32, ptr, uint32)`; never copy TTT's Linux-marshaller reason-first order into a direct call.
- Synthetic `cs_win_panel_round`/`nextlevel_changed` fired from JS never reach OTHER plugins' JS subscribers (isolate-borrow rule) — fine for client UI; consumers detecting round end must use `round_end`. Documented in the demo.
- ABI-tail collision with `feat/writeconvar` (§4.2) — second-to-merge re-tails.
