# SwitchTeam (non-lethal team switch — `Player.switchTeam`) — design spec

**Date:** 2026-07-16
**Status:** design (autonomous decision — no human gate) → implementation plan next
**Scope:** gamedata + core op + shim sig-resolve/call + `@s2script/cs2` surface + demo. **One new engine fact (a self-resolved byte-signature); no hooks, no detours, no marshalling changes.**
**Consumer:** the TTT port — gap-analysis slice #7 (`feat/switchteam`, "Role→team without killing the player", Low risk).

## 1. Problem & consumer

TTT calls CounterStrikeSharp's `CCSPlayerController.SwitchTeam` at exactly three sites, always with `CsTeam.Terrorist` (2) or `CsTeam.CounterTerrorist` (3), never Spectator/None (spectators are guarded out before every call):

1. **Role assignment at round start** (`RoleIconsHandler.OnAssigned`, RoleIconsHandler.cs:118-128) — detective→CT, innocent/traitor→T, on **alive** players. This site is why the primitive must be non-lethal: a jointeam-semantics change here would slay the whole server every round start. TTT then **defers all pawn work to the next world update and re-fetches the pawn** — its own comment (:134-138) records that SwitchTeam can respawn the pawn.
2. **Body identification mid-round** (`BodyPickupListener.OnIdentify`, BodyPickupListener.cs:61-70) — a **dead** player's controller moves T→CT (scoreboard/win-condition truth), then TTT re-forces `PawnIsAlive = false`. The primitive must work on a dead controller and must not require them to stay respawned.
3. **Role reveal at round end** (`RoundTimerListener.revealRoles`, RoundTimerListener.cs:134-146) — bulk innocent→CT on mixed alive/dead players so the win panel shows innocents on CT.

The load-bearing property is **NOT killing the player** (CSSharp's documented contract: "the player will remain alive and keep their weapons", vs `ChangeTeam` = jointeam semantics, "will usually cause a player suicide/loss of weapons"). TTT does **not** rely on pawn-entity stability — it already re-resolves the pawn a frame later, which is exactly s2script's `EntityRef`/`Pawn` model.

The gap-analysis row's word "deferred semantics missing" is a mislabel carried over from our old RE note (§2); the accurate requirement is the slice-table wording: **role→team without killing the player**.

## 2. The RE — the real SwitchTeam, and the ghost of the wrong one

**History (recorded in `gamedata/core.gamedata.jsonc:202-214` and the changeteam slice):** during the changeteam slice, CSSharp's borrowed "SwitchTeam" signature resolved on our build to the **wrong function** — a setter that queues the deferred `m_bSwitchTeamsOnNextRoundReset` halftime swap, with **no immediate move** (live-gate-proven: no player moved). Same trap family as CSSharp's ChangeTeam vtable index (slot 101 = a `ret` stub here; the real ChangeTeam is slot 102).

**Verified fact this slice builds on:** the signature

```
55 48 89 E5 41 54 49 89 FC 89 F7
```

resolves **UNIQUE at va 0x1525f40** on our 2000875 `libserver.so` — adjacent to ChangeTeam (@0x1524770; vtable siblings), corroborated by **both** SwiftlyS2 and current CSSharp shipping this exact pattern. ABI: `void CCSPlayerController::SwitchTeam(this /*rdi*/, unsigned int team /*esi*/)` — the prologue saves `this` into r12 (`49 89 FC`) and moves `team` esi→edi (`89 F7`), consistent with that signature. Per `docs/re-strategy.md` this is a *hint corroborated → re-validated on OUR binary*, not a bare borrowed constant.

**Doctrine compliance at load (the ChangeTeam pattern, verbatim):**

- The entry ships as a `"SwitchTeam"` gamedata signature (`resolve: "direct"`); `ResolveSigValidated` re-validates **exactly-1 match** in the pinned `libserver.so` PF_X range on every boot and SCREAMS via `GamedataResult` on the update treadmill.
- The call site `.text`-range-guards the resolved pointer (reuses `s_serverText`/`s_serverTextSize`).
- **Degrade-per-descriptor:** unresolved/moved sig → `s_pSwitchTeam` stays null → the op no-ops with a named boot reason; the core native no-ops without the op (pinned by an in-isolate test). Never a crash.

## 3. SwitchTeam vs ChangeTeam semantics — and the spectator dispatch

| | `changeTeam` (shipped) | `switchTeam` (this slice) |
|---|---|---|
| Engine fn | `CCSPlayerController::ChangeTeam` (vtable 102, CTMDBG-xref-resolved) | `CCSPlayerController::SwitchTeam` (@0x1525f40) |
| Semantics | jointeam: immediate move, usually kills/loses weapons | **non-lethal**: alive + weapons kept; pawn MAY be respawned |
| Teams | 0..3 incl. Spectator | T/CT (2/3) native; 0/1 handled by dispatch (below) |
| Dead controller | works | works (pure team move — the TTT body-identify path) |

**Decision — spectator/none dispatch: YES, `team <= 1` dispatches to ChangeTeam, in the shim.** Both SwiftlyS2 and CSSharp do exactly this (the engine SwitchTeam is CS:GO-lineage T/CT-only; its behavior for 0/1 is unvalidated and possibly wrong). Dispatching keeps the primitive total over 0..3 with zero new RE (`s_pChangeTeam` is a static in the same translation unit), avoids a silent-no-op footgun, and is framework parity — while the `.d.ts` still points spectator intent at `spectate()`. TTT never passes 0/1, so this is a safety net, not a load-bearing path. (Deviation from research R1, which leaned reject/no-op — see §8.)

`team` is bounds-checked 0..3 shim-side; out-of-range is a no-op, like changeTeam.

## 4. THE decision: synchronous, not deferred

**Verdict: `Player.switchTeam` calls the engine function SYNCHRONOUSLY on the calling frame — an exact structural sibling of `changeTeam`. No deferral queue.**

Reasoning:

1. **The deferred alternative is literally the function we already rejected.** The engine's own "deferred switch" (`m_bSwitchTeamsOnNextRoundReset`) is the wrong-function trap from the changeteam slice; TTT needs the team visibly moved *now* (round-start assignment, mid-round scoreboard truth for body identify). A framework-side defer-to-next-frame would additionally break the synchronous read-back (`p.teamNum` immediately after the call) that the changeTeam surface already established, and would add queue machinery with zero consumer benefit.
2. **The re-entrancy hazard is real but already handled — and already shipped.** SwitchTeam can respawn the pawn, which fires engine-side events (`player_spawn`, `player_team`) *inside* the call. Core holds `HOST.borrow_mut()` across all JS, so when a JS handler calls `switchTeam`, any event dispatch the engine triggers mid-call re-enters core and hits the `try_borrow_mut` graceful-skip guards: those nested events are **skipped for JS subscribers, not crashed on**. This is the identical hazard profile `changeTeam` shipped with (ChangeTeam can kill the pawn → death events mid-call) and has been live-proven benign. The caveat is documented on the API: events fired by the engine during a JS-originated `switchTeam` do not re-dispatch to JS handlers on that frame.
3. **TTT's own defensive pattern is the consumer-side answer**, and our model already enforces it: defer pawn-dependent work to the next frame and re-resolve `player.pawn` (serial-gated `EntityRef` — a respawned pawn simply resolves to the new entity or `null`, never a stale pointer).

## 5. API shape (exact TS surface)

CS2 game package (`@s2script/cs2`) — the engine fact is a CS2 class function, so nothing touches `@s2script/sdk`. In `packages/cs2/index.d.ts`, directly after `spectate()`:

```ts
  /**
   * NON-LETHAL team switch between Terrorist (2) and CounterTerrorist (3) via the sig-resolved
   * CCSPlayerController::SwitchTeam: the player stays alive and keeps their weapons (vs `changeTeam`,
   * which has jointeam semantics and usually kills). Works on DEAD controllers too (a pure
   * scoreboard/win-condition team move). CAVEAT: the engine MAY respawn the pawn during the call —
   * re-resolve `player.pawn` on the next frame before any pawn write. Game events the engine fires
   * inside the call do not re-dispatch to JS handlers on that frame (re-entrancy skip). For None (0) /
   * Spectator (1) this dispatches to `changeTeam` (CSSharp/SwiftlyS2 parity) — prefer `spectate()`.
   * Serial-gated; a no-op if the ref is stale or the signature is unresolved. Bounded 0..3 engine-side.
   */
  switchTeam(team: number): void;
```

TTT parity mapping: `player.SwitchTeam(CsTeam.CounterTerrorist)` → `player.switchTeam(3)`; the RoleIconsHandler defer-then-refetch → `nextFrame()`/`delay()` + `player.pawn` re-read; the BodyPickup `PawnIsAlive = false` re-force → `player.pawnIsAlive = false` (already writable via the generated schema accessor).

## 6. Architecture — where everything lives, and the ABI append point

Carbon copy of the changeteam slice's seven production touchpoints:

| Piece | Where | Notes |
|---|---|---|
| `"SwitchTeam"` sig entry | `gamedata/core.gamedata.jsonc` (next to `"ChangeTeam"` :215) | data, not code; doctrine comment carries the wrong-function history |
| `PlayerSwitchTeamFn` alias + `pub player_switch_team` field + `s2_player_switch_team` native + `set_native` + mock-struct entries + degrade test | `core/src/v8host.rs` | engine-generic `(idx, serial, team)` — no CS2 name in core |
| `s2_player_switch_team_fn` typedef + struct field | `shim/include/s2script_core.h` | the C twin of the ABI |
| `SwitchTeam_t` typedef + `s_pSwitchTeam` + op fn (serial-gate, bounds, `.text` guard, spec-dispatch) + Load-time resolve block + `ops.player_switch_team` wiring | `shim/src/s2script_mm.cpp` | every CS2 fact stays here |
| `Player.prototype.switchTeam` | `games/cs2/js/pawn.js` (next to `changeTeam` :109) | dist pawn.js is a CONCAT — never raw-cp |
| `switchTeam(team)` decl | `packages/cs2/index.d.ts` | §5 |
| `.changeset/switchteam.md` | `@s2script/cs2` **minor** (precedent: changeteam → 0.5.0) | no sdk changeset — nothing in `packages/sdk` changes |

**ABI append point: after `transmit_stats`** — the current `S2EngineOps` tail on origin/main (`core/src/v8host.rs:372`, `shim/include/s2script_core.h:386`), with the standard comment `// --- switchteam slice (APPENDED after transmit_stats; order is the ABI; do not reorder above) ---` in **both** files, plus `player_switch_team: None` in **both** test mock op-structs (the two `transmit_stats: None` sites) and the `ops.player_switch_team = …` wiring after `ops.transmit_stats` (:3589).

**Tail-collision hazard (flagged):** PRs #67 (`gamerules_terminate_round`), #71 (`voice_set_muted`/`voice_get_muted`), #76 (`usermsg_hook_*`), #80 (`player_respawn`) all append at the same tail — and #67/#71 are already stale (they still append after `usercmd_clear_subtick`, pre-transmit). `S2EngineOps` has no size/version handshake; a missed re-tail after someone else merges is a **silent function-pointer misdispatch**, not a compile error (Rust's exhaustive struct literals catch a missing *field*, not a wrong *order* vs the C twin). At every `gt restack`: re-read the trunk tail and re-append in the Rust struct, the C twin, and the wiring, together.

## 7. Boundary check

*Would it still be true on a different Source 2 game?* Core sees only "move controller (idx, serial) to team N via an engine op" — generic (the change_team precedent: "Engine-generic here; only the resolving signature is game-specific"). `CCSPlayerController`, the byte-signature, the spectator-dispatch policy, and the `CEntityHandle` reconstruction are shim-side. The API lives in `@s2script/cs2`. `make check-boundary` + `scripts/test-boundary-nameleak.sh` stay green by construction.

## 8. Deviations from the research

- **Spectator handling:** R1 recommended reject/no-op for team 0/1; this design dispatches to ChangeTeam instead (§3) — CSSharp/SwiftlyS2 parity, zero new RE, no silent-no-op footgun. TTT exercises neither path.
- **R1/R2's open question "which function is the real SwitchTeam"** is CLOSED by the verified fact (§2): unique @0x1525f40 on 2000875, SwiftlyS2+CSSharp-corroborated. No RTTI vtable walk needed this slice; the boot gate still re-validates per treadmill doctrine.
- **Gap-analysis wording:** the "deferred semantics" label in GAP-ANALYSIS.md:50 is corrected to "non-lethal semantics" by this spec (§1); the port tracker should be updated when the slice lands.

## 9. Deferred (do NOT build ahead)

- **A framework `player_team`-consistency guarantee** (does the engine update win-condition team counts mid-round?) — verified observationally at the live gate, not abstracted into API surface.
- **Absorbing the dead-controller alive/respawn side effect** — if the live gate shows SwitchTeam resurrects a dead controller's pawn, the primitive still ships as the raw engine behavior + a documented caveat; the TTT port replicates TTT's own `pawnIsAlive = false` re-force (already writable). No shim-side re-force magic.
- **`dropActiveWeapon`** — the adjacent deferred item from the items slice; unrelated RE, stays deferred.

## 10. Live-gate plan (the main loop's job — sniper build + Docker gate)

All bot-provable (a bot is a real controller with a real pawn). Deterministic, mirrors TTT's three scenarios:

1. **Boot:** `SwitchTeam resolved @…` in the log; `GamedataResult` gate green (uniqueness re-validated).
2. **Alive switch (role-assignment shape):** `sm_switchtest` — alive bot T→CT: `teamNum` changed **immediately** (synchronous read-back), **no kill** (no `player_death`, health unchanged), **weapons survived** (count before/after), and the pawn-respawn probe (pawn `EntityRef` index:serial before vs after, logged either way — this settles the respawn question with live evidence).
3. **Dead switch (body-identify shape):** `sm_slay` a bot, then `sm_deadtest` — dead T→CT: `teamNum` moved, `pawnIsAlive` still false after the call (or the resurrect side effect is logged and documented, §9).
4. **Bulk reveal shape:** `sm_revealtest` — every T→CT in one frame, no crash, `RestartCount=0`.
5. **Side-by-side sanity:** `sm_spectest` (changeteam-demo) still behaves — proves the two siblings resolved to *different* functions.

**Risks / STOP conditions:**

- **STOP: the sig resolves non-unique or fails on the gate's binary** (a CS2 update between 2000875 and gate day) → the boot gate names it; re-run the resolve against the new binary before shipping — never widen the mask blind.
- **STOP: the alive switch kills the player or doesn't move the team** → we have the wrong function again (the changeteam-slice failure mode). Do not ship; re-open the RE (RTTI walk near ChangeTeam slot 102).
- **Dead-controller respawn side effect** → not a STOP; document + keep the TTT-side `pawnIsAlive` re-force pattern (§9).
- **Re-entrancy:** a `switchTeam` from inside an event handler must never crash (the `try_borrow_mut` guards); the demo's commands run from the command dispatch path, which exercises exactly this.
