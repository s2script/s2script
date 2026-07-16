# Voice control (server-side voice mute + onVoice) — design spec

**Date:** 2026-07-16
**Status:** design (autonomous decision — no human gate) → implementation plan next
**Scope:** core + shim + `@s2script/sdk` clients surface + basecomm migration + demo. **No gamedata, no sig-scan, no game-package change.**
**Consumer:** the TTT port (`PlayerMuter.cs` / `MuteReward.cs`) + basecomm's `sm_mute` (currently best-effort, self-documented UNVERIFIED).

## 1. Problem & consumer

TTT's `PlayerMuter` needs exactly four things (verified against `/home/gkh/projects/TTT/TTT/CS2/GameHandlers/PlayerMuter.cs` and `RTD/Rewards/MuteReward.cs`):

1. **An `onVoice` notification** — "this client is transmitting voice", per slot. Both TTT consumers mute **lazily inside this handler** (dead player talks mid-round → mute + one-time reminder), so the hook is REQUIRED, not optional.
2. **An idempotent per-slot voice mute/unmute** — `VoiceFlags |= Muted` / `&= ~Muted` in CSSharp terms. Semantics: the muted client's **outgoing** voice is silenced for **every** receiver.
3. **An isMuted read** — used only to gate the one-time reminder message.
4. Clear-on-`player_spawn` and clear-all-on-round-end — pure consumer code on the existing `Events.on` + `Clients.all()` surface.

basecomm's `sm_mute` today writes `m_bHasCommunicationAbuseMute` (schema), whose live voice effect is **UNVERIFIED** per its own header (`plugins/basecomm/src/plugin.ts:6-10`) and the deferred-live-tests memory. This slice replaces its enforcement with the real mechanism.

## 2. The mechanism — THE key decision

**Verdict: NOT reuse-only. A new engine touchpoint is required — but it carries ZERO new sig-scanned engine facts.**

The reuse-only path (a cs2-package Voice API over `EntityRef.writeInt32` + `notifyStateChanged` on `m_iVoiceFlags`, mirroring round-control's team-scores reuse) is **impossible**:

- **`m_iVoiceFlags` is not a schema field.** It appears nowhere in our schema dump (`games/cs2/gamedata/schema-catalog.json` — only `m_voicePitch`, `m_voiceEndTimestamp`), nowhere in the generated accessors, and nowhere in CSSharp's own schema use. CSSharp's `player.VoiceFlags` (the exact property TTT compiles against) is **framework-internal C++ state** (`CPlayer::m_voiceFlag`, a `uint8` — `player_manager.h:133`), read/written by plain natives (`NativeAPI::Get/SetClientVoiceFlags`), never a game-class field.
- The only engine-persisted voice-ish bit we have (`m_bHasCommunicationAbuseMute`) is the wrong/unverified mechanism (possibly matchmaking/GC-driven).
- **There is no persistent engine mute bit to write.** The engine keeps a per-(receiver, sender) listen matrix (`IVEngineServer2::Get/SetClientListening`, eiface.h:329-330), but the game layer (Source-lineage `CVoiceGameMgr`) **re-sets it recurrently** — a one-shot write does not persist. Proven by design: neither CSSharp nor Swiftly ever calls `SetClientListening` directly (grep of both trees: zero hits); both only **rewrite the in-flight `bListen` parameter** in a PRE SourceHook.

**The proven mechanism** (CSSharp `voice_manager.cpp:26-66`; Swiftly `entrypoint.cpp:79` + `voicemanager/manager.cpp:29-85` — identical design, independently):

1. **Enforcement:** `SH_DECL_HOOK3(IVEngineServer2, SetClientListening, SH_NOATTRIB, 0, bool, CPlayerSlot, CPlayerSlot, bool)` — a PRE hook on the **already-acquired** `s_pEngine` (`shim/src/s2script_mm.cpp:594`, live-proven via KickClient/ClientPrintf/GetClientXUID). When the **sender**'s framework-side mute flag is set and `bListen` is true, `RETURN_META_VALUE_NEWPARAMS(MRES_IGNORED, bListen, &IVEngineServer2::SetClientListening, (receiver, sender, false))` — the engine's own implementation still runs, with the parameter swapped. The flag array lives **in the shim** (plain `uint8_t[64]`): the hook fires per (receiver, sender) pair per game voice refresh — up to O(n²) — so the hot path must be an allocation-free array lookup with **zero FFI and zero JS**.
2. **Notification:** `SH_DECL_HOOK1_void(ISource2GameClients, ClientVoice, SH_NOATTRIB, 0, CPlayerSlot)` (eiface.h:619, "TERROR: A player sent a voice packet") — a POST hook, the **7th sibling** of the six lifecycle SourceHooks already installed on the already-acquired `m_gameClients` (`s2script_mm.cpp:2709-2723`). It forwards `s2script_core_dispatch_client_event("voice", slot)` into the existing name-keyed `CLIENT_MUX` spine — **zero new core plumbing** beyond the prelude registration. It fires per received voice packet (tens/sec while talking), so the shim **throttles per-slot to ≤1 dispatch/second** (the first packet of a transmission always dispatches — TTT's lazy mute lands immediately).

No per-packet `CLCMsg_VoiceData` detour is needed (basecomm's header guessed that path; the CSSharp evidence supersedes it — no detour, no protobuf, no sig).

### RE-doctrine compliance (the borrowed-layout problem)

No byte-signature or offset is borrowed — both touchpoints are interface virtuals from the pinned SDK. But the **vtable positions are borrowed layout facts**, and `SetClientListening` sits in a **hand-patched region** of `eiface.h` (an `#if 0 // Don't really match the binary` block + `unk301/unk302` before it, `unk401-403` after — :318-335). This is exactly the ChangeTeam-slot-102-vs-101 class of drift. Per the doctrine, the slice self-validates behaviorally:

1. **First-fire semantic validation** (the checktransmit pattern): the first time our `SetClientListening` hook fires, sanity-check the args (both slots in `[0, 64)`). Garbage → set the degraded flag, log a LOUD named reason, and never rewrite. Sane → log the observed (receiver, sender, listen) once — this also settles the "does CS2 call it recurrently?" unknown with live evidence.
2. **A Get/Set round-trip** (genuinely self-resolving, one-shot, when the **2nd client goes active** — bots count, so the 2-bot docker gate exercises it): `orig = GetClientListening(a,b)` → `Set(a,b,!orig)` → assert `Get == !orig` → `Set(a,b,orig)` → assert `Get == orig`. `GetClientListening` is the adjacent virtual (:329); a mismatched/shifted vtable region cannot plausibly pass the two-direction toggle-and-restore. Pass → `VOICE VALIDATION: ... round-trip OK`. Fail → the voice descriptor degrades with a **named reason**; the framework keeps running; `voiceMuted` writes become logged no-ops (return 0 across the op).
3. **Recurrence hedge:** enforcement normally rides the game's periodic `SetClientListening` refresh (Source1 cadence: 0.3s; TTT-in-production implies CS2 recurs too). To remove that as a load-bearing assumption for the *mute* direction, `voice_set_muted(slot, true)` — **only after the round-trip has passed** — proactively calls `s_pEngine->SetClientListening(r, slot, false)` for every currently-valid receiver `r`. Our own pre-hook sees these calls harmlessly (sender is muted → param already false; `NEWPARAMS` does not re-enter). Unmute relies on the engine's next refresh (a laggy unmute is benign; a laggy mute is the feature failing).

`ClientVoice`'s slot rides the same validation the six proven siblings give the interface (a reorder would break all seven together), plus its own in-range check before dispatch.

## 3. API shape (exact TS surface)

Engine-generic (both interfaces are Source2), so it extends `@s2script/sdk`'s clients module — **not** `@s2script/cs2`. PascalCase types, camelCase members, per the locked convention.

```ts
// packages/sdk/clients.d.ts — additions
export declare class Client {
  // ... existing ...
  /**
   * Server-side voice mute: while true, this client's OUTGOING voice is silenced for every receiver
   * (the SetClientListening rewrite). Framework state, NOT an engine field: cleared automatically on
   * disconnect (a reused slot never inherits a mute); persists across map changes while connected.
   * If the voice descriptor is degraded (hook/validation failure — named reason in the server log),
   * setting is an inert no-op and reads stay false.
   */
  voiceMuted: boolean;
}
export declare const Clients: {
  // ... existing ...
  /**
   * Fires while a client transmits voice (ISource2GameClients::ClientVoice). Throttled shim-side to at
   * most one dispatch per client per second; the FIRST packet of a transmission always fires, so a
   * lazy mute-on-talk lands immediately. Handlers should be idempotent. Never fires for bots.
   */
  onVoice(handler: (client: Client) => void): void;
};
```

**Why this shape and not `VoiceFlags`:** TTT uses only the `Muted` bit; CSSharp's `Speak_All/Team/ListenAll/ListenTeam` cascade needs team reads inside the shim hook (a boundary hazard) and a re-derived, unvalidated voice-priority cascade. Per build-by-risk, v1 ships the boolean; a future flags surface can wrap the same shim state without ABI breakage (new ops append). **Why `onVoice` exists at all:** the flag alone cannot express TTT's lazy "mute on the first talk attempt + reminder" UX — both TTT consumers register the listener.

TTT parity mapping: `player.VoiceFlags |= VoiceFlags.Muted` → `client.voiceMuted = true`; `(VoiceFlags & Muted) != Muted` → `!client.voiceMuted`; `Listeners.OnClientVoice` → `Clients.onVoice`; spawn/round-end clears → existing `Events.on("player_spawn"/"round_end")` + `Clients.all()`.

## 4. Architecture — new ops, where everything lives

| Piece | Where | Why |
|---|---|---|
| `s_voiceMuted[64]` flag array + both hook bodies + throttle + validation | **shim** (`s2script_mm.cpp`) | The `SetClientListening` hook is O(n²)-per-refresh hot: the flag must be a local array read, zero FFI (the inverse of the BAN_CACHE core-side shape, which is right for once-per-connect, wrong here). Cleared per-slot in `Hook_ClientDisconnect`. |
| `voice_set_muted(slot, on)` / `voice_get_muted(slot)` ops | `S2EngineOps` tail — **appended after `usercmd_clear_subtick`** (`shim/include/s2script_core.h:375` / `core/src/v8host.rs:359`), wired at `s2script_mm.cpp:3476` | JS sets/reads flags via ops; the hook never calls JS. All FIVE ABI places (C typedef+member, Rust alias+field, both test op-structs at v8host.rs ~:11042 / ~:11695). |
| `"voice"` client event | existing `CLIENT_MUX` spine (`ffi.rs:96` → `dispatch_client_event` → `v8host.rs:553`) | Name-keyed — zero new core dispatch code; the shim's `Hook_ClientVoice` calls the existing FFI export. |
| `__s2_voice_set_muted`/`__s2_voice_get_muted` natives + prelude `Client.prototype.voiceMuted` + `Clients.onVoice` | `core/src/v8host.rs` (clients prelude, ~:1854-1861) | Engine-generic module surface. |
| `.d.ts` | `packages/sdk/clients.d.ts` | Engine-generic (§3). |
| basecomm migration + demo | `plugins/basecomm`, `examples/voice-demo` | Consumers. |

**ABI-tail collision (flagged):** the unmerged transmit and round-control stacks append at the same `usercmd_clear_subtick` tail. `S2EngineOps` has **no size/version handshake** (copied by value at `s2script_core_init`) — whichever stack merges second MUST re-tail in all five places on rebase; a missed re-tail is a **silent function-pointer misdispatch**, not a compile error. The plan's global constraints carry this.

## 5. Boundary check

*Would it still be true on a different Source 2 game?* Yes for every core/sdk piece: `IVEngineServer2::SetClientListening`, `ISource2GameClients::ClientVoice`, `CPlayerSlot`, and "a per-slot outgoing-voice mute" are Source2 concepts (the SDK headers declare them engine-wide). No CS2 identifier crosses the C ABI or appears in `core/src`; no schema name is involved anywhere. The demo's dead-check (`Player`/`pawn.health`) and basecomm are CS2 **plugins** — the correct side of the boundary. Gates: `check-core-boundary.sh` + `test-boundary-nameleak.sh` stay green.

## 6. Deferred (do NOT build ahead)

- **Per-(receiver, sender) `ListenOverride` matrix** (CSSharp `Listen_Mute/Listen_Hear`) — TTT-unused; the hook signature naturally accommodates it later as state-only.
- **`Speak_All/Team/ListenAll/ListenTeam` flags** — needs an `m_iTeamNum` read inside the shim hook (game-boundary hazard) + the voice-priority cascade; Muted-only v1.
- **`vban` client self-mute mirroring** (CSSharp `voice_manager.cpp:103-118`) — only matters if we ever force listen=**true**; we never do.
- **Text-chat muting** — already solved (`Chat.onMessage` `Handled`, basecomm gag); do not rebuild.
- **Proximity/positional voice** (`SetClientProximity`, eiface.h:331) — out of scope.
- **`m_bHasCommunicationAbuseMute`** — investigation closed as *superseded*, kept in basecomm only as a cosmetic best-effort scoreboard indicator.
- **Mute-source composition** (basecomm admin mute vs a gamemode's round-scoped mute fighting over one bit) — the primitive stays a single shared bit, last-writer-wins; basecomm re-asserts its SteamID-keyed set on `onPutInServer` (this slice) and consumers needing more keep their own bookkeeping. A per-source refcount is a consumer-layer follow-up if it ever bites.

## 7. Live-gate plan

Bots never transmit voice and cannot hear — so the gate is **tiered**, mechanism-first (the 6.6 damage-slice precedent):

**Tier 0 — bot-provable (blocks the PR):**
1. Boot log: both hooks installed (`voice: SetClientListening hook installed`, `voice: ClientVoice hook installed (throttled notify)`).
2. `bot_add` ×2 via rcon → the 2nd `ClientActive` runs the one-shot round-trip → `VOICE VALIDATION: Get/SetClientListening round-trip OK (slots a,b)`. **FAILED → STOP** (§8).
3. `sm_voicetest <botslot> 1` → reply `voiceMuted=true`; `sm_voicetest <botslot> 0` → `false` (op + flag + read-back proven). Watch for the hook's first-fire log line — record whether/how often CS2 calls `SetClientListening` (the recurrence evidence, engine-paced).
4. `RestartCount=0`, no panic — proves the proactive-apply direct calls (incl. bot-slot receivers) are safe.

**Tier 1 — needs ONE human client (run if a human is available; otherwise logged deferred):** connect, hold push-to-talk → `[voice-demo] onVoice slot=…` lines at ≤1/s (throttle proven); die (`sm_slay @me`) and talk → lazy mute + reminder chat fires once; respawn → unmute log. Verifies `ClientVoice` end-to-end.

**Tier 2 — needs TWO human clients (DEFERRED, flagged loudly):** the audible proof — set `sv_full_alltalk 1`, verify the talker is heard (baseline), mute them, second human confirms **silence**, unmute, confirm audible again. This is exactly the already-logged "basecomm voice-mute effect" deferred live test; update that memory entry to point at this primitive. Without the baseline convar step, CS2's own dead-voice rules (`sv_deadtalk` etc.) can fake a pass or a fail.

## 8. Risks / STOP conditions

- **STOP: round-trip validation fails or first-fire args are garbage** → the hand-patched eiface region has drifted on our binary. Do not ship enforcement; the descriptor stays degraded (named reason), `voiceMuted` inert-by-contract. Next step would be RTTI/xref work on the real vtable — a new slice decision, not an improvisation.
- **STOP: any crash attributable to a direct `SetClientListening`/`GetClientListening` call** (validation or proactive apply) → remove the direct-call paths, keep the pure hook-rewrite mechanism (CSSharp ships exactly that), re-gate.
- **Recurrence is engine-paced:** if the first-fire log shows CS2 calls `SetClientListening` rarely/never, mute latency rests on the proactive apply (mute) and unmute latency may lag until the next engine refresh — measure at the gate, document what's observed. Do NOT escalate to a `CLCMsg_VoiceData` detour without a new slice (nobody, CSSharp included, needed it).
- **Hot-path discipline:** no allocation, no FFI, no JS in `Hook_SetClientListening`; `Hook_ClientVoice` dispatches under the existing `try_borrow_mut` re-entrancy guard and is throttled — never put enforcement in a JS handler.
- **Slot hygiene:** flags are slot-keyed; `Hook_ClientDisconnect` clears the slot's flag + throttle stamp, or the next occupant inherits a mute (the admin-cache slot-reuse lesson). SteamID persistence is consumer-level (basecomm re-asserts on `onPutInServer`).
- **Unload hygiene:** both hooks removed in `Unload` (the sibling pattern). Forced-false listen values already stored in the engine are restored by the engine's own next refresh; if the gate shows no recurrence, note that an unload-while-muted leaves the pair silenced until map change (accepted, logged).
- **The flag is inert without the hook** — the API must not pretend: degraded ⇒ op returns 0, setter is a logged no-op, reads stay false, boot log carries the named reason.
