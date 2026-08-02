# Deferred-dispatch queue — delivering re-entrant notify dispatches one drain later

**Status:** Approved — ready for planning.
**Audience:** core/shim maintainers; anyone debugging "my plugin stopped getting events".
**Builds on:** `#63`'s `reentrant_dispatch_is_currently_dropped` test and its doc comment
(`core/src/v8host.rs:12050-12105`), which specifies this fix and is this slice's acceptance
tripwire; `fan_out`/`Channels<H>` (A4); the four existing pending queues in `ffi.rs:63-67`.
**Unblocks:** A5b (`docs/superpowers/specs/2026-08-01-gamedata-tiering-design.md` §9).

---

## 1. The problem

Core holds `HOST.borrow_mut()` across all JS. When a JS handler causes the engine to synchronously
dispatch back into core — `Events.fire()` from an event handler, or an engine call whose side effect
fires an event — the inner dispatch's `try_borrow_mut` fails and the dispatch is **silently
dropped**. No error, no log. It presents only as "my plugin stopped getting events".

`#63` investigated and rejected `v8::CallbackScope`: every `NewCallbackScope` impl in rusty_v8
149.4.0 needs either `&mut Isolate` — the borrow we do not have — or a live V8 handle, and the
re-entrant dispatch arrives through the C ABI with none in hand. Reconstructing one from a stashed
`*mut Isolate` would alias a live `&mut`. That door is closed.

**This blocks A5b.** Retiring `Respawn` and `TerminateRound` from core requires their engine calls
to be made from JS, but both fire events synchronously. The shim's current hand-rolled drains exist
precisely to run those calls *outside* the borrow so the resulting `player_spawn` reaches plugins —
a JS `nextFrame` callback runs *inside* `dispatch_onframe`'s borrow and cannot reproduce that.

## 2. Scope: notify-only

Only **notify-only** dispatches become deferrable.

A pre-hook returns a `HookResult` the engine consumes synchronously; there is no answer to give a
frame later. Pre-hooks keep today's graceful skip, and the shipped `.d.ts` for each affected
capability (`@s2script/sdk/damage`, `/usermsg`, `/usercmd`, and `Events.onPre`) gains an explicit
note that a pre-hook re-entered from within a dispatch is not delivered. This is a permanent,
documented limitation — `#63` reached the same conclusion — not a gap to close later.

This excludes `Damage.onPre`, `UserMessages.onPre`, and the usercmd hook by construction, which also
puts their block-scoped raw views out of scope: **no raw pointer is ever queued.**

## 3. Ownership: the shim owns the queue

Core detects the failed borrow but owns no dispatch payload. Every dispatch *originates* in the
shim, which still has its arguments on the stack, and a game event's data lives in an engine-owned
`IGameEvent` valid only for the duration of the call.

So the signal inverts: **core's notify dispatchers return `Deferred` instead of returning silently,
and the shim queues the replay.**

```
shim                                   core
----                                   ----
dispatch_game_event(name)          -->  try_borrow_mut fails
                                   <--  DEFERRED
dup = mgr->DuplicateEvent(cur)
queue.push({ kind, name, dup, args })

[next GameFrame, HOST free]
s_currentEvent = dup
dispatch_game_event(name)          -->  runs; handlers read REAL fields
mgr->FreeEvent(dup)                     (RAII — also on throw)
```

Nothing about an `IGameEvent` is ever represented in Rust, and scalar-payload dispatches
(client lifecycle, entity lifecycle, map start) need no duplication at all — the shim replays them
from arguments it already holds.

### 3.1 The C-ABI change

There are 16 `s2script_core_dispatch_*` entries; only the **notify-only subset** changes. The
pre/collapsing entries — `game_event_pre`, `damage`, `usercmd`, `usermsg`, `concommand`,
`command_listeners`, `game_frame` — are out of scope per §2 and keep their signatures untouched.
Enumerating the exact deferrable set against the current tree is the first task of the plan, not a
guess here.

Each entry in that subset gains a `Deferred` return signal: entries already returning a `HookResult`
int use a reserved sentinel outside the `HookResult` range, and `void` entries become `int`. This
moves the shim boundary, so **the slice requires a live gate** — per CLAUDE.md, a shim-boundary
change is never proven by unit tests alone.

### 3.2 The queue entry is a tagged union

The deferrable entries do not share a signature, so a queue entry is a tag plus that entry's own
arguments — not a single generic payload. The shim owns this union precisely because it is the side
that already has those arguments typed and on the stack. Adding a future deferrable dispatch means
adding a variant, which is the same shape as adding a `fan_out` channel and is deliberately explicit
rather than a `void*` blob.

`s_currentEvent` save/restore **nests**: `event_create`/`event_fire` already save and restore it
(`s2script_mm.cpp:606-616`), so a deferred handler that itself calls `Events.fire()` composes with
the drain's own swap without special handling. The drain must use the same save/restore discipline
rather than assuming `s_currentEvent` is null on entry.

`DuplicateEvent` and `FreeEvent` are adjacent slots on `IGameEventManager2`, an interface the shim
already sig-resolves and calls (`CreateEvent`/`FireEvent`). Per `docs/re-strategy.md` the hl2sdk
header is a **hint, not a number**: both slots are validated at load (resolved pointer inside
`libserver.so`'s `.text`) and the descriptors degrade by name if validation fails. If they cannot be
resolved, event deferral degrades to today's drop with a named reason at boot — scalar dispatches
still defer.

## 4. The queue

**One FIFO queue**, so a deferred `player_death` and a deferred client-disconnect keep their
relative order. Splitting by payload type would leave their order undefined.

**Double-buffered.** The drain swaps buffers before replaying, so a dispatch deferred *by* a
deferred handler lands in the *next* drain rather than extending the current one. Without this, a
handler that re-fires its own event spins the frame forever. This is the `RunFrameHooks`
`frame_queue`/`frame_actions` swap.

**Bounded**, with a named overflow log naming the dropped dispatch, mirroring `kRespawnPendingMax`.
An unbounded queue turns a plugin bug into an OOM. Overflow drops the *newest* and says so.

**Drained at the top of `Hook_GameFramePre`**, before anything enters JS — verified: the damage
self-test is the first thing in that hook that reaches core, and the drain precedes it. `HOST` is
provably free there.

**Timing, stated precisely.** A dispatch deferred during frame *N* is replayed at frame *N+1*'s
`GameFrame` — one frame later, not the same frame. This matches the existing hand-rolled
`s_pendingRespawn`/`s_pendingTerminate` drains exactly, which is what lets A5b retire them without
changing observable timing. `#63`'s doc comment says the same thing: the queue converts "silently
dropped" into "delivered one frame later".

**Liveness.** A plugin may unload between push and drain; core's existing per-subscriber `is_live`
check covers that. An entity in a scalar payload may die; the existing books-gated resolve already
degrades to `null`. Neither needs new machinery.

## 5. Testing

**The primary acceptance test already exists.** `reentrant_dispatch_is_currently_dropped`
(`v8host.rs:12081`) was written in `#63` as this slice's tripwire. Its assertion flips from
`__ran == 0` to `__ran == 1` and it is renamed to `reentrant_dispatch_is_delivered_next_drain`.

Added:
- **Nested defer** — a deferred handler that re-fires lands in the next drain, not the current one.
  Asserts the frame does not spin and the second delivery still arrives.
- **Overflow** — pushing past the cap logs a named drop and does not grow the queue.
- **Ordering** — two dispatches deferred in one frame replay in push order.
- **Pre-hook stays skipped** — a re-entrant pre-hook is not queued (it would have no one to answer).

**Live gate** (a shim-boundary change is not provable offline):
1. A real `player_death` re-fired from a handler reaches a second plugin **with correct field
   values** — the zeroed-fields failure the duplication exists to prevent.
2. The deferred delivery lands in the same frame as the defer.
3. `FreeEvent` runs on the throwing path (a deferred handler that throws does not leak).

## 6. Non-goals

- Pre-hook deferral (§2) — permanent, documented.
- Deferring dispatches carrying raw views (damage, usercmd, usermsg) — out by construction.
- Retiring the shim's existing `s_pendingRespawn`/`s_pendingTerminate` drains. They keep working
  unchanged; **A5b** removes them once its ops move to gamedata descriptors.
- A general "run this later" API for plugins. `nextFrame` already exists.
