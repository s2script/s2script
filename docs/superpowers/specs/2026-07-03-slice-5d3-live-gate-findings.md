# Slice 5D.3 — event actionability — LIVE-GATE RESULTS (de_inferno) — PASSED

Server: Docker CS2, de_inferno, past the boot window. All three write-side capabilities proven, plus a
re-entrancy defect found + fixed.

## Evidence

```
[s2script] [demo] onLoad (5D.3 event actionability)
[s2script] [demo] fired player_hurt (from onLoad) ok=true
[s2script] [demo] PRE round_start timelimit 0->4242 (Handled)
[s2script] [demo] POST round_start timelimit=4242
```
(0 `already borrowed` panics after the re-entrancy fix.)

- **FIRE** — `Events.fire("player_hurt", {...})` → `ok=true` (`CreateEvent` + `FireEvent` succeeded).
- **The FireEvent SourceHook fires on a real engine event** — `PRE round_start ...` proves the hook
  catches organically-fired events, which **confirms the one flagged risk (the SDK `IGameEventManager2`
  vtable index for `FireEvent` matches CS2)** — no wrong-vfunc; the hook lands on `FireEvent`.
- **MODIFY** — the pre-hook `ev.setInt("timelimit", 4242)` changed the live event; the POST handler read
  `timelimit=4242`.
- **BLOCK = suppress-broadcast (SM parity)** — the pre-hook returned `HookResult.Handled`, yet the POST
  handler (a server-side `AddListener` subscriber) STILL fired and saw the modified value → the event
  was processed server-side (broadcast to clients suppressed), not fully killed.

`round_start` (deterministic via `mp_restartgame`) was used instead of `player_hurt` because
`CS2_MAXPLAYERS=2` + a large map makes bot combat unreliable; the mechanism is identical for any event.

## Defect found + fixed (re-entrancy)

The first demo fired the event from INSIDE the `round_start` handler. That re-enters the dispatch while
the V8 isolate is already borrowed (`HOST.borrow_mut()` held by the outer dispatch) → **`RefCell already
borrowed` panics** at `v8host.rs:2072` (`dispatch_game_event` notify) and `:2141`
(`dispatch_game_event_pre`). `catch_unwind` caught them (no crash) but logged panics and lost the nested
re-dispatch. This is a genuine defect that `Events.fire` (new in 5D.3) surfaced.

**Fix:** a `try_borrow_mut()` graceful-skip guard in BOTH event-dispatch paths — on re-entrancy the
nested JS dispatch is skipped (pre → allow/`0`, notify → no-op); the engine-side fire still happens; no
panic. Covered by the in-isolate test `reentrant_dispatch_skips_gracefully_no_panic` (core 122/122).

**The re-entrancy LIMITATION is by design and documented:** a JS-triggered `Events.fire` cannot
re-dispatch to JS subscribers, because ALL JS runs while the isolate is borrowed (onLoad, event handlers,
frame handlers) — so the fired event reaches the ENGINE (clients + C++ listeners / other plugins) but not
this framework's own JS `on`/`onPre` subscribers on the same pass. This matches the observation that the
`onLoad` fire logged `ok=true` but produced no PRE/POST for the fired event. (SourceMod's dispatcher is
not a single global borrow, so it re-dispatches; matching that is a deeper follow-up, out of scope here.)

## Other T5 polish folded in
- The T3-review Minor: `Hook_FireEventPre`'s unused `bDontBroadcast` param annotated `[[maybe_unused]]`
  (the codebase idiom; shim has no `-Werror` so it was harmless, but consistent).
