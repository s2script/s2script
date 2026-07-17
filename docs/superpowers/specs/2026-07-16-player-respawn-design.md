# Player respawn — design spec

**Date:** 2026-07-16
**Status:** design (autonomous) → implementation plan `docs/superpowers/plans/2026-07-16-player-respawn.md`
**Scope:** TWO new self-resolved engine facts (`CCSPlayerController::Respawn`, sig + RTTI-vtable-membership-validated; and `CBasePlayerController::SetPawn`, sig + `.text`-validated — required pre-step, live-gate-determined) + one deferred engine op + `Player.respawn(): boolean` in `@s2script/cs2`. TTT gap #5 (severity High), slice 3 of the TTT port. Worktree: `s2script-respawn`, branch `feat/respawn`. See §2.3 for the SetPawn resolution + the game-rules behavioral finding.

## 1. Problem & consumer

The TTT port needs to bring dead players back to life mid-round — the one hidden TTT dependency the round-control spec (§6) explicitly deferred ("fresh RE, its own sig"). TTT calls `player.Respawn()` at exactly 4 sites, **always on the CONTROLLER** (CSSharp's pawn-level `Respawn` is `[Obsolete]` and does nothing), and **always deferred** via `Server.NextWorldUpdate` — never synchronously from the triggering callback:

| TTT site | Condition at call time |
|---|---|
| `RoundTimerListener.OnRoundStart` (COUNTDOWN, :41-49) | every player with health ≤ 0 on T/CT |
| `RoundTimerListener.OnRoundStart` (IN_PROGRESS, :58-65) | same — dead T/CT players |
| `LateSpawnListener.OnJoin` :37-41 / `GameState` :49-54 | a just-joined player **unconditionally** (possibly pawnless, team None) and dead non-spectators on state change |
| `TeamChangeHandler.onJoinTeam` :53-56 | dead player changing team while no game in progress — called from **inside a command hook** |

Two consumer facts shape the whole design:

1. **The dominant case is a DEAD player on a playing team; but the primitive is also invoked on fresh/pawnless and potentially already-alive controllers** and must degrade to a graceful no-op — CSSharp itself returns silently when `PlayerPawn.Value == null`.
2. **TTT never depends on synchronous completion.** `CS2Game.cs:81-93` documents that a late-joiner's scheduled respawn may not have run by `StartRound` and gates participants on `IsAlive`. A primitive that internally defers to the next frame is semantically sufficient — and (per §4.1) mandatory for us anyway.

Port shape: `for (const p of Player.all()) if (!p.pawnIsAlive && onPlayingTeam(p)) p.respawn()` — a 1:1 translation of every TTT site, with the `NextWorldUpdate` deferral **built into the op** so consumers can't get it wrong.

## 2. The engine fact

### 2.1 `CCSPlayerController::Respawn` — the forbidden pattern, and its doctrine-compliant replacement

**CSSharp resolves Respawn from a BARE vtable offset** (`CCSPlayerController_Respawn`: `{windows:272, linux:274}`, NO signature — unlike SetPawn/ChangeTeam/CommitSuicide which ship byte-sigs). A raw borrowed vtable index is precisely the doctrine's forbidden second-row constant (`docs/re-strategy.md` Rule 1) and precisely the failure class that already bit us twice: sm_slay's borrowed slot 400 was a getter; CSSharp's ChangeTeam slot 101 was a `ret` stub on our build (real fn = slot 102). **The index never ships. It was consumed OFFLINE as a finding aid (re-strategy Rule 3) and discarded.**

**Offline RE — DONE (2026-07-16, this worktree, pinned build 2000875** — `steam.inf` ServerVersion=2000875, `docker/cs2-data/.../libserver.so`, `.text` PF_X window va `0x9e5b3c` size `0x1976863`):

1. **RTTI-resolved the `CCSPlayerController` primary vtable offline** (the same Itanium walk `s2vtable::GetVTableByName` does at runtime: typeinfo-name `"19CCSPlayerController"` @va `0x81ef30` → typeinfo @`0x24879b8` → primary vtable (offset-to-top 0) fn[0] slot @va `0x2488800`, slot pointers recovered from `.rela.dyn` `R_X86_64_RELATIVE` addends).
2. **Slot 274 = va `0x14f0ce0`** — and it is the **LAST fn slot of the primary vtable** (slot 275 holds the next sub-vtable's offset-to-top `-2728`, slot 276 its typeinfo pointer): structurally consistent with Respawn being the newest-added virtual.
3. **Methodology self-check:** the same offline vtable read gives slot 102 = va `0x15241f0`, and our shipped, live-proven `ChangeTeam` prologue sig scans to **exactly 1 hit at `0x15241f0`** on this binary — the RTTI walk and the sig pipeline agree on a known-good function.
4. **Disassembly of `0x14f0ce0`** is a clean **nullary controller method** — `push rbp; mov rbp,rsp; push r12; push rbx; mov rbx,rdi; call <helper>; test rax,rax; jz …; mov rdi,rax; mov rax,[rax]; call [rax+0xC98]; test al,al; jz …; mov rdi,rbx; call <same helper>; …` — fetch an object off the controller, early-branch on a bool virtual (an alive-check shape), re-fetch, dispatch. Consistent with `void CCSPlayerController::Respawn(this /*rdi*/)` and with CSSharp's nullary invoker.

**The SHIPPED artifact** is a self-derived masked prologue byte-sig (gamedata `resolve:"direct"`), masking the two volatile call rel32s, the jz disp, and the virtual-call vtable disp, keeping the stable structural bytes — **validated UNIQUE (exactly 1 match) at `0x14f0ce0`** in the PF_X window:

```
55 48 89 E5 41 54 53 48 89 FB E8 ? ? ? ? 48 85 C0 74 ? 48 89 C7 48 8B 00 FF 90 ? ? ? ? 84 C0
```

Resolution + call = byte-for-byte the ChangeTeam/CommitSuicide pipeline: `ResolveSigValidated` (0 = moved, >1 = ambiguous, named `GamedataResult` line either way) → `.text` range guard before every call → direct call with the serial-gated controller as `this`.

**Prototype (Linux SysV):** `void CCSPlayerController::Respawn(this /*rdi*/)` — no other args. The shim typedef is `typedef void (*Respawn_t)(void* controller);`.

### 2.2 Semantic load-validation: RTTI vtable-membership (`Respawn.vtable-member`)

Uniqueness alone is NOT enough — the round-control slice proved a sig can match exactly once at the WRONG function. Respawn has no unique log string to xref (unlike ChangeTeam's `CTMDBG`), so the substitute semantic anchor is **RTTI vtable membership**, and it is **mandatory**: after `ResolveSigValidated` passes, the shim runtime-resolves the `CCSPlayerController` primary vtable via `s2vtable::GetVTableByName("libserver.so", "CCSPlayerController")` (the trace-slice/CNavPhysicsInterface precedent, `s2script_mm.cpp:2338/:3419`) and asserts the sig-resolved address **is present among the primary vtable's fn slots** (walk slots while the slot value lies in libserver `.text`; the first non-`.text` value is the sub-vtable header = end of the primary fn slots). This binds the match to "a genuine virtual method of CCSPlayerController" — the exact check that would have caught sm_slay's and ChangeTeam's drifted indices. Reported as its own boot-gate descriptor line `Respawn.vtable-member`; on failure `s_pRespawn` stays null with the named reason *"sig-resolved address is NOT a member of the RTTI-derived CCSPlayerController primary vtable (unique-but-WRONG match — the borrowed-sig trap)"*. The passing path logs the matched slot number — a free treadmill breadcrumb (drift from 274 on a future build is informational, not fatal).

**Treadmill recipe (goes in the gamedata comment):** if the prologue moves, re-resolve by RTTI-walking the primary vtable offline (the re_respawn.py recipe: typeinfo name → typeinfo → offset-to-top-0 vtable → `.rela.dyn` addends) and re-derive the mask from the new last-slots' disassembly; the vtable-member gate then re-anchors it. CSSharp's slot number is a HINT for which slot to look at first, never a shipped number.

### 2.3 SetPawn — REQUIRED, and self-resolved (live-gate-driven; the design's Plan C, executed)

CSSharp's `Respawn()` calls `CBasePlayerController::SetPawn(pawn, true, false)` FIRST (a "Call To Arms"-era fix: a dead/observing controller's active `m_hPawn` points at the observer pawn). This slice originally deferred that pre-step (Plan A = Respawn-alone, Plan B = a pawn.js `m_hPawn` handle-write, Plan C = resolve SetPawn) and let the live gate pick. **The live gate picked Plan C.** On 2000875:

- **Respawn-alone (Plan A) is insufficient** — it clears the death overlay but never spawns the player (observed).
- **A raw `m_hPawn` handle-write (Plan B) is insufficient too** — SetPawn does more than the handle assignment (observer teardown, the pawn→controller backref, dirty flags).
- So **SetPawn is genuinely required and was self-resolved** on our binary. `CBasePlayerController::SetPawn` is a **4-arg NON-VIRTUAL** function `void(void* controller, void* pawn, bool b1, bool b2)`, called `(controller, playerPawn, true, false)` — verbatim what **SwiftlyS2** (`player.cpp:345`) and CSSharp both do. CSSharp's borrowed sig (`… 41 57 41 89 CF`) has **0 hits** on 2000875 (a `push r14` was inserted, moving the ecx-save to r14d), so we ship a **self-derived masked prologue** validated UNIQUE at va `0x15ef580` — and **SwiftlyS2's up-to-date gamedata ships a byte-identical sig**, an independent corroboration. Resolved by unique-match + `.text` guard (non-virtual → no vtable-member gate); unresolved → the op degrades to 0. The two bool args are the borrowed ABI values SwiftlyS2/CSSharp pass; a treadmill note lives in the gamedata comment.

**Live-gate behavioral finding (important):** the engine's `Respawn` **honors the game's respawn rules**. On a plain **competitive mid-round** server it **no-ops** — killed players stay dead, and `Respawn` returns without setting `m_bPawnIsAlive` (confirmed via an in-drain before/after diagnostic: SetPawn + Respawn both execute on valid pointers, alive stays 0). The engine *can* respawn on that same server — it auto-respawns in **warmup** — so this is a game-state gate, not a broken call. This is the correct, expected behavior and matches why maul/SwiftlyS2's identical call works for them: they call it in gamemodes that permit respawn. **TTT is built around controlled respawns** (its `LateSpawnListener`/`RoundTimerListener` respawn in its own game flow), so `Respawn` fires in TTT's context. A direct positive revival was not obtainable on the test box because standard modes either forbid respawn (competitive) or auto-respawn instantly (warmup/DM) — it will be confirmed when the TTT port runs on its own rules.

### 2.4 The alive-guard offset — schema, not borrowed

`CCSPlayerController.m_bPawnIsAlive` (already codegen'd: `games/cs2/js/schema.generated.js:402` `pawnIsAlive`) guards the already-alive case JS-side, and its offset is passed to the op as an opaque third arg so the shim can **re-check at drain time** (closing the enqueue→drain 1-frame TOCTOU — the round-control `rules_ptr_off` shape: no CS2 name crosses the C ABI). Schema offsets self-resolve via the schema catalog — not a borrowed fact.

## 3. API shape (`@s2script/cs2` — packages/cs2/index.d.ts + games/cs2/js/pawn.js)

One method. No options bag, no batch API (consumers compose `Player.all()` + health/team reads), and **no `Pawn.respawn`** — upstream pawn-level Respawn is deprecated/dead, and the controller is the stable identity precisely when the player is dead (`player.pawn` is `Pawn | null`, null while dead).

```ts
// packages/cs2/index.d.ts — Player interface, inserted after spectate() (:125)
/** Respawn this (dead) player via the self-resolved CCSPlayerController::Respawn (byte-sig +
 *  RTTI-vtable-membership load-validated). QUEUED: the engine call executes on the NEXT engine
 *  frame, outside the JS isolate borrow, so the resulting player_spawn reaches EVERY plugin's
 *  handlers — including the caller's. Safe from inside event/command handlers; no nextFrame
 *  wrapping needed. Returns false when degraded: the player is already alive, the ref is stale,
 *  or the Respawn descriptor failed its boot gates. */
respawn(): boolean;
```

```js
// games/cs2/js/pawn.js — after Player.prototype.spectate (~:115)
Player.prototype.respawn = function () {
  if (this.pawnIsAlive === true) return false;               // alive-guard (engine behavior on an
  if (typeof __s2_player_respawn !== "function") return false; // alive pawn is unproven — see §6)
  var aliveOff = __s2_schema_offset("CCSPlayerController", "m_bPawnIsAlive");
  return __s2_player_respawn(this.ref.index, this.ref.serial, aliveOff) === 1;
};
```

**Why `boolean`, deviating from `changeTeam(): void`:** changeTeam is synchronous fire-and-forget on an always-valid path; respawn is queued AND multiply-degradable (unresolved descriptor, stale ref, already alive) — TTT never reads the return, but a silent-`void` queued op is undiagnosable in the field. `setName`'s boolean precedent applies.

## 4. Architecture & dispatch

### 4.1 One new op, deferred execution (the re-entrancy decision — round-control §4.1 precedent, adopted verbatim)

The respawn path fires `player_spawn` (+ pawn entity-lifecycle onSpawn) **synchronously inside the engine call**. Core holds `HOST.borrow_mut()` across all JS; a synchronous call from any JS handler (exactly TTT's flows — one site calls from inside a command hook) would re-enter dispatch, hit `try_borrow_mut` graceful-skip, and **every plugin would silently miss that player_spawn**. TTT itself hooks player_spawn for loadouts — a first-consumer-visible silent failure. So deferral is baked into the op, not left to callers:

- `s2_player_respawn(idx, serial, alive_off) -> int` **enqueues** into a pending set and returns 1; 0 on degrade (unresolved sig, stale controller at enqueue, set full).
- **Multi-entry pending set** (fixed array, capacity 130, deduped by handle) — a deliberate deviation from round-control's single-slot latest-wins: TTT's round-start loop respawns MANY players in one JS dispatch, and dropping all but the last would be a correctness bug. A round ends once; a frame respawns many.
- A dedicated `Hook_GameFrameRespawnDrain` SourceHook pre-hook on `ISource2Server::GameFrame` — installed **eagerly at Load iff both gates passed** (the round-control eager-install rationale: never mutate the hook chain from inside a frame dispatch; one branch/frame is negligible), removed at Unload — drains the set **outside the JS borrow**: per entry it re-derefs the handle (serial-gated at drain, not just enqueue), re-checks `m_bPawnIsAlive` via `alive_off` (skip if the player came alive in between — round restart etc.), `.text`-guards `s_pRespawn`, and calls `s_pRespawn(controller)`. The resulting player_spawn flows through the normal FireEvent pre-hook → core dispatch → all subscribers.
- Cost: one frame (~15ms) of latency — semantically irrelevant for a respawn, and exactly TTT's own `NextWorldUpdate` semantics, so the port is a strict simplification (no wrapping needed).
- **Drain sharing:** if round-control (#67) merges first with its `Hook_GameFrameRoundDrain`, respawn MAY fold its drain into that hook at restack time; the design assumes its own hook (mechanically trivial either way — decide at restack, note in the PR body).

### 4.2 ABI append point — after `transmit_stats`; three-way collision watch

`S2EngineOps` is positional `#[repr(C)]` with **no size/version handshake** — field order IS the ABI; a mirror divergence silently shifts every later op into the wrong dispatch. This worktree's confirmed tail (both mirrors) is the checktransmit trio ending `transmit_stats` (`shim/include/s2script_core.h:386`, `core/src/v8host.rs` struct tail, shim wiring `s2script_mm.cpp:3589`). Append **exactly one** op after `transmit_stats` in ALL lockstep sites — C typedef + struct field, Rust typedef + field, **both full in-test `S2EngineOps` literals** (~`v8host.rs:11167` and `mock_event_ops` ~`:12028`; `transmit_test_ops` uses `..mock_event_ops()` spread — no edit), and the shim `ops.` wiring — same commit, verified by the ordered-field-name parity diff:

```c
typedef int (*s2_player_respawn_fn)(int idx, int serial, int alive_off);
```

**Collision flag:** PR #67 (round-control, `gamerules_terminate_round`) and PR #71 (voice, `voice_set_muted`/`voice_get_muted`) are unmerged and anchored at the OLDER `usercmd_clear_subtick` tail (their structs predate checktransmit); `feat/writeconvar` (5 cvar fields) is anchored older still. Whichever merges later re-tails: **right before submit, re-check which of #67/#71/writeconvar landed and re-anchor `player_respawn` after the newly merged tail**, updating all lockstep sites in the same commit.

### 4.3 Op semantics (shim)

```c
/* enqueue:  1 = queued (executes next GameFrame outside the JS borrow), 0 = degraded */
static int s2_player_respawn(int idx, int serial, int alive_off);
/* drain:    per entry — re-deref (serial-gate), alive re-check at controller+alive_off
 *           (alive_off < 0 => skip the re-check, the JS guard already ran), .text guard, call */
void S2ScriptPlugin::Hook_GameFrameRespawnDrain(bool, bool, bool);
```

Consume-before-call in the drain (the engine call can re-enter gamerules/event machinery). Dedupe on enqueue makes double-respawn-same-frame idempotent. `(idx, serial)` is the **controller** EntityRef (Player.ref) — the change_team resolution path, not commit_suicide's pawn path; §2.1's disassembly confirms the receiver is the controller.

## 5. Boundary check (core vs shim vs @s2script/cs2) — litmus per piece

| Piece | Home | Litmus: true on another Source 2 game? |
|---|---|---|
| Op slot + `__s2_player_respawn` native + degrade test | core | Yes as plumbing: "serial-gate an entity, queue a gamedata-resolved call with one opaque bool-field offset" — zero CS2 names cross the ABI (the `player_change_team`/`rules_ptr_off` precedent). |
| `Respawn` sig, `Respawn_t`, pending set + drain hook, vtable-member gate | shim + gamedata | No (CS2 function) → shim owns it; the sig is regenerable data; `GetVTableByName` is an engine-generic helper. |
| `"CCSPlayerController"` / `"m_bPawnIsAlive"` / `"m_hPawn"` / `"m_hPlayerPawn"` strings, `Player.respawn` | games/cs2 + packages/cs2 | No (CS2 class/field names) → game package only. |

Gates: `make check-boundary`, `./scripts/test-boundary-nameleak.sh`, `./scripts/check-plugins-typecheck.sh` (additive .d.ts).

## 6. Safety matrix

| Input state | Guard | Result |
|---|---|---|
| Already-alive pawn | JS `pawnIsAlive === true` pre-guard + at-drain `alive_off` re-check | `false` / silent skip. Engine behavior on an alive pawn is unproven lore (slot-274's body has an alive-check-shaped branch, but do not rely on it) — probe ONCE at the live gate so the guard's necessity becomes documented fact. |
| No pawn / dead | none — **this is the target state**; the controller persists while dead | queued |
| Fresh late-joiner (pawnless, team None) | engine-side: the resolved fn's own null-pawn path (CSSharp returns silently on null pawn) | must not crash — live-gate item |
| Stale/disconnected ref | serial-gate at enqueue AND drain (`s2_deref_handle`) | `false` / dropped |
| Slot/index out of range | same resolve path bounds-checks | `false` |
| Bot | fine by mechanism — controller op, no netchannel (the bot-crash class is networking paths); ChangeTeam/CommitSuicide bot-proven | live-gate verified anyway |
| Descriptor unresolved / failed gate | `s_pRespawn == nullptr` → op returns 0; core native without ops returns 0 | `false`, framework keeps running |

## 7. Degrade chain (named-reason, fail-closed)

(a) Load: `sigs.find("Respawn")` absent → `GamedataResult("Respawn", false, "signature absent from gamedata")`; `ResolveSigValidated` 0/>1 → named FAILED line; vtable-membership fail → `Respawn.vtable-member` FAILED with the unique-but-wrong reason; all surface in the `GAMEDATA VALIDATION: N ok, M FAILED` banner. Either failure leaves `s_pRespawn` null and the drain hook uninstalled. (b) Call: null fn / stale / full set → 0 → `respawn() === false`. (c) Drain: stale or re-alive → per-entry skip (logged), never a crash. (d) Test: `player_respawn_degrades_without_op` mirrors `player_change_team_degrades_without_op` (`v8host.rs:11445`). Residual borrowed facts: **none at runtime** — slot 274 was offline-only; the alive/pawn-handle offsets are schema-self-resolving; Plan B's "handle-write ≙ SetPawn" equivalence is a behavioral claim guarded solely by the live gate (documented above).

## 8. Deferred / out of scope (log in docs/PROGRESS.md)

- **`SetPawn` resolution (Plan C)** — only if Plan A and Plan B both fail live; fresh RE, its own descriptor.
- **`Pawn.respawn`** — deprecated upstream; no consumer.
- **Batch `Player.respawnAll` / filters / force-respawn-alive flag** — TTT composes loops in plugin code; a force path becomes a v2 question only if a consumer intentionally respawns a live player.
- **Windows gamedata** — linuxsteamrt64 only, like every existing descriptor.

## 9. Live-gate plan (MAIN LOOP's job — sniper build + Docker CS2; out of the implementation plan's scope)

Demo `examples/respawn-demo` (commands `sm_respawn <slot>`, `sm_respawnall`; loggers on `player_spawn`/`player_death`).

1. **Boot markers:** `gamedata OK Respawn` AND `gamedata OK Respawn.vtable-member` (+ the logged slot number — expect 274); `GAMEDATA VALIDATION: N ok, 0 FAILED`.
2. **THE decision point (Plan A vs B):** kill a bot (`sm_slay`), then `sm_respawn <slot>` **from the command handler** (a JS dispatch — the hazard path) → bot visibly respawns: `pawnIsAlive` flips true, health > 0, position = a fresh spawn point (not the death spot), AND the demo's own `player_spawn` logger fires (the deferred-drain proof). If the bot is NOT re-activated → flip pawn.js to Plan B (m_hPawn pre-write) and re-run; if B also fails behaviorally → STOP, Plan C.
3. **Loop parity:** `sm_slay @all` then `sm_respawnall` → every dead player respawns in the same frame batch (multi-entry set proof).
4. **Guards:** `sm_respawn` on an alive bot → `false`, nothing happens; on a disconnected/stale slot → `false`; alive-probe: once, with the JS guard temporarily bypassed in a scratch build, observe engine behavior on an alive pawn — document, do not ship the bypass.
5. **No-crash soak:** respawn cycles across a round restart + a `changelevel`.

## 10. Risks / STOP conditions

- **STOP if `Respawn.vtable-member` fails at boot** on the pinned build — the sig landed somewhere wrong; re-run the RTTI recipe, never weaken the gate to ship.
- **STOP if gate item 2 respawns the bot but the demo's player_spawn logger does NOT fire** — the drain isn't actually outside the borrow; fix the drain, don't ship one-plugin-visible respawn.
- **Plan A insufficiency is NOT a stop** — it's the designed pivot to Plan B (zero new facts, pawn.js-only change).
- Slot-274 vtable-member walk assumes every primary-vtable fn slot points into libserver `.text` (a cross-module thunk would truncate the walk early). CCSPlayerController is concrete on this build (verified offline: slots 0..274 all resolve); if a future build breaks this, the gate fails LOUD (fail-closed), not wrong.
- The `+0xC98` virtual and the helper called in Respawn's body are unidentified — behavioral facts guarded by the live gate only; treadmill note in the gamedata comment.
- ABI-tail collision (#67 / #71 / writeconvar) — later-merger re-tails (§4.2).

## 11. Deviations log

1. **No `Pawn.respawn`** (the slice brief mentioned it): upstream pawn-level Respawn is `[Obsolete]`/no-op; the controller is the identity while dead. Player-level only.
2. **`respawn(): boolean`, not changeTeam's `void`:** queued + multiply-degradable needs a detectable failure (setName precedent).
3. **SetPawn NOT resolved in this slice** (research option (a) demoted): CSSharp's SetPawn sig has **0 hits on 2000875** (stale hint — fresh RE required), and Respawn-alone may suffice; the live gate decides, Plan B costs zero engine facts.
4. **Multi-entry pending set** vs round-control's single-slot latest-wins: TTT respawn loops are many-per-frame.
5. **`alive_off` third op arg** (beyond the minimal `(idx, serial)`): closes the enqueue→drain TOCTOU for one opaque int, the `rules_ptr_off` precedent.
6. **RE shipped pre-resolved** (no spike task in the plan): the sig, its uniqueness, the vtable slot, and the ChangeTeam methodology cross-check were all derived and validated offline in this worktree against the pinned 2000875 binary.
