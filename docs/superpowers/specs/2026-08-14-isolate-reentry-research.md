# Research: isolate re-entry as one spine

**Date:** 2026-08-14
**Status:** findings — not a slice plan. Local census + HOST/V8 nest study, then `deep-research-2` (Partial). Do not implement from this file until a spine is locked and the mechanism in §5.1 is spiked.
**Question:** What would it take to fix “a plugin’s engine call must be visible to other plugins’ hooks and events” **across the board**, instead of per-hook queues and skips?

Related: pickup-gates design (`giveNamedItem` must fire `onCanAcquire`); deferred-dispatch queue (#63); gamedata-tiering §9.2 (respawn/terminateRound `nextFrame` drains); switchteam design (sync on purpose).

---

## 1. Verdict

This is **one host rule**, not a family of hook bugs.

Core holds `HOST.borrow_mut()` across all JS. Any engine callback into core while that is true hits the same `try_borrow_mut` in `fan_out_inner`. Notify events can be replayed next frame. Pre-hooks cannot. Secret `nextFrame` drains in `pawn.js` were a workaround that **does not** restore pre-hooks and **does** break “the world has changed when this returns.”

**Do not** put `giveNamedItem` on a next-frame queue. Other frameworks do not. It would not run `onCanAcquire`. It would make give as dishonest as `respawn()` is today.

**Do not** “drop `HOST`” as a literal `RefCell` drop. The outbound native does not hold `HOST`. The outer `HandleScope` does. A second `HandleScope::new(&mut isolate)` is the aliasing `#63` closed.

The remaining door, still on pinned rusty_v8 **149.4.0**, is narrower than nest-the-isolate and wider than another queue:

> Outbound natives already have a live V8 handle (`FunctionCallbackInfo` / `Local<Context>`). `#63` only closed “C-ABI inbound, no handle, don’t fake an isolate pointer.” It did **not** evaluate stashing that live handle for the FFI window and building a `CallbackScope` for inbound `dispatch_hook` **without** taking `HOST`.

That is the only spine that can make `giveNamedItem` fire `onCanAcquire` **and** keep give synchronous. Recursion policy must be per-call (`bypassWith`, “give from inside CanAcquire is skipped”), not “everything nests.”

Until that spine is spiked and live-gated, pickup-gates PR1 is the thunk + subscribe surface. Whether a JS give can fire it is a host question.

---

## 2. The one mechanism

```2736:2745:core/src/v8host.rs
/// Core holds `HOST.borrow_mut()` across ALL JS, so a handler that causes the engine to
/// synchronously dispatch back into core hits a failed `try_borrow_mut` on the inner dispatch.
/// ...
/// Only **notify-only** dispatches can carry this. A pre-hook returns a `HookResult` the engine
/// consumes synchronously; there is no answer to give a frame later
```

`HOST` is `RefCell<Option<Host>>`. `Host` is `{ isolate, context }`. Every inbound JS entry (`dispatch_onframe`, `fan_out_inner`, `eval`, `frame_async_drain`) takes that borrow for the whole `func.call`.

JS natives **do not** touch `HOST`. They run as V8 `FunctionCallback`s on the `PinScope` V8 already pushed (`__s2_subscribe` says this explicitly; `engine_call_invoke` is the same). While plugin A is inside `giveNamedItem`:

```
dispatch_*              HOST.borrow_mut()
  HandleScope(&mut isolate) + ContextScope(A)
    A's JS
      FunctionCallback PinScope     // native; no HOST of its own
        engine FFI
          thunk → dispatch_hook
            fan_out_inner try_borrow_mut → FAIL
```

`dispatch_hook` already SAVE/RESTOREs `ACTIVE_HOOK` because “a handler can make the engine call another hooked function.” The hook **views** are nest-ready. The isolate **lock** refuses the inner fan-out.

---

## 3. Census — three masks on one hole

### 3.1 Deferred-dispatch queue (notify only)

Replay next `Hook_GameFramePre`, isolate free: game events, client lifecycle, map start, entity create/spawn/delete, cvar change. A replay that itself returns deferred is dropped and logged, never re-queued.

Pre-hooks stay silent `Continue`: `Events.onPre`, `onDamage`, `onOutput`, `UserMessages.onPre`, chat, `onClientCommand`, usercmd. `fakeCommand` of an `sm_*` command silently skips the target plugin’s JS handler.

### 3.2 Secret `nextFrame` drains

`Player.respawn` and `GameRules.terminateRound` queue in `pawn.js`. The drain runs inside `frame_async_drain`, which **holds** `HOST`. So:

- `Events.on("player_spawn"|"round_end")` is replayed (one more frame late).
- `Events.onPre` for those names **never** runs for plugin-issued calls (gamedata-tiering §9.2a, written as permanent).
- The world has not changed when the method returns.

`switchTeam` was **explicitly not** queued (2026-07-16) because TTT must read `teamNum` on the next line. Same contract `giveNamedItem` needs.

### 3.3 `bypassWith` latch

JS `terminateRound` / `respawn` arm a latch so `onTerminateRound` / `onRespawn` do not fire. That is SourceMod `g_pIgnoreTerminateDetour` / `blockhook`: a plugin’s own call is not an engine event. Pickup-gates spec forbids this on `CanAcquire`.

Any other JS path to the same address (plugin `engine:calls`) still re-enters; `note_reentrant_skip` names it once; the engine proceeds unhooked.

### 3.4 Outbound calls that matter for ports

| call | today | author lie |
|---|---|---|
| `giveNamedItem` | sync | item exists; `onCanAcquire` skipped |
| `switchTeam` / `changeTeam` / `slay` | sync | state changed; mid-call events next frame / pre silent |
| `respawn` / `terminateRound` | queued | state **not** changed; pre dead; own hook bypassed |
| `create` / `spawn` / `remove` | sync | lifecycle notify next frame |
| `Events.fire` | sync | `on` next frame; `onPre` silent |
| `Server.command` / `setCvar` | queued next frame | isolate free when it runs — hooks **do** fire |

Other frameworks do **not** secretly queue Give / Respawn / TerminateRound. `RequestFrame` / `Server.NextFrame` is opt-in and in the author’s face.

---

## 4. Why `#63` is not a closed door for this

Pinned: `v8 = "149.4.0"` (`core/Cargo.toml`). v150+ local-exec TLS cannot link the `-shared` cdylib.

`#63` rejected `CallbackScope` because **every** `NewCallbackScope` impl in rusty_v8 149.4.0 needs either `&mut Isolate` (the borrow we do not have) **or a live V8 handle**. The re-entrant dispatch arrives through the C ABI with none of those. Reconstructing a handle from a stashed `*mut Isolate` aliases a live `&mut`.

That is still true for **engine-originated** inbound (no JS native on the stack).

It is **not** the situation for **plugin-originated** outbound: the native’s `FunctionCallback` **is** a live handle. `CallbackScope` is already used where rusty_v8 has a handle (`promise_reject_cb` from `PromiseRejectMessage`) and **must not** touch `HOST`.

The unevaluated path: outbound native publishes that handle on a TLS stack for the FFI window; `dispatch_hook` builds `CallbackScope` from it and does **not** take `HOST`.

---

## 5. Three spines (not per-hook patches)

| | Idea | Sync give + CanAcquire? | Honest read-back? | Risk |
|---|---|---|---|---|
| **A. Nested CallbackScope on outbound FFI** | Publish live handle; inbound hook dispatch does not take `HOST` | Yes | Yes | Recursion; rusty_v8 scope plumbing; must keep `#63` for no-handle C-ABI |
| **B. Native HOST-free drain** (cookies/ws window, or old C++ GameFrame drain) | Run the engine call after JS has released the isolate | Only if the call is **native** and we accept next-frame world change | **No** for give/respawn/terminate | Same porting ghosts as today’s queues |
| **C. Keep skip, never secret-queue** | Every skip is a named, author-visible no-op / null | No | Yes (world matches return) | Restrict cannot vote; give-then-strip remains |

**B is the wrong default** for give / respawn / terminate / switchTeam. It is what we already did for respawn/terminate and then documented as losing pre-hooks forever. It is what the user correctly rejected as jarring and as the source of porting oddities.

**C is honest and insufficient** for pickup gates. The spec requires the give to fire the gate, not “give succeeded, restrict strips later.”

**A is the SourceMod/ModSharp/Swiftly shape**, implemented under V8’s constraints instead of CLR nest. Scope it to outbound engine FFI, not “HOST is reentrant.” Do **not** wrap `Events.fire` blindly or the deferred-queue recursion brake goes away.

Making `HOST` itself reentrant (depth counter / `UnsafeCell`) is a worse version of A: every inbound path could nest, including C-ABI entries with no handle. That is `#63` plus UB.

---

## 6. Recursion policy (required if A ships)

`ACTIVE_HOOK` nesting is already correct. Unbounded engine → JS → engine → JS is not.

| Path | Policy |
|---|---|
| `giveNamedItem` from `onCanAcquire` | Skip and name (pickup spec §4 / live-gate 4). Depth cap as backstop. |
| JS `terminateRound` / `respawn` | Keep `bypassWith` (own hook does not fire). `round_end` / `player_spawn` **notify** may nest or stay deferred — decide in the design, do not silently undo the queue’s double-buffer. |
| `Events.fire` from an event handler | **Do not** put on spine A by default. Today deferred + double-buffered so a re-fire cannot spin the frame. |
| Unlatched `engine:calls` to a hooked address | Keep `note_reentrant_skip` if no published handle. |
| `await` in a pre-hook | Still illegal. Nest is sync JS on the same stack. |

---

## 7. What a proving slice looks like (after spine lock)

Not another hook PR. One host change + one live-gate that would have caught **both** of today’s holes:

1. Plugin A `giveNamedItem("weapon_negev")` while plugin B’s `onCanAcquire` denies defIndex 28 → A gets `null`, B’s handler ran, player never sees the gun.
2. Plugin A `player.respawn()` (or a sync respawn if we un-queue it under A) → `Events.on("player_spawn")` **and** a test `onPre` see it on the same call if we choose nest for events; if we keep notify deferred, the gate at least proves the **pre-hook** path.

Existing tripwire: `a_reentrant_hook_dispatch_is_skipped_and_named`. A real A slice flips that for “outbound native + published handle” and **keeps** it for unlatched / no-handle C-ABI re-entry.

Pickup-gates PR1 does not wait on this. It ships the thunk and `ctx.items.onCanAcquire`. JS give remaining invisible is this spine, not a missing items wrapper.

---

## 8. Out of scope

- `Handle<T>` / JS pointers for `CEconItemView` (handle-vs-pointer research).
- Plugin-held function pointers / `Engine.hook`.
- Upgrading rusty_v8 past 149.4.0 to get a nicer scope API.
- Per-hook `nextFrame` queues as a substitute for A.
- `await` inside pre-hooks.

---

## 9. Coverage / still open

- rusty_v8 149.4.0 `CallbackScope` constructors were not re-read line-by-line in this note; `#63`’s comment is the in-tree authority. The outbound-handle door is **reasoned**, not spiked.
- Whether publishing `FunctionCallbackInfo` across the C ABI (shim thunk) is enough, or whether the handle must stay in Rust TLS for the whole FFI, is unproven.
- Blast radius of un-queuing `respawn` / `terminateRound` once A exists is a design question (read-back vs `bypassWith` vs `Events.onPre` for plugin-issued calls).

---

## 10. What `deep-research-2` verified vs left open

**Agreed (survived verification):** the census; notify vs pre-hook split; secret `nextFrame` drains lose `onPre`; `bypassWith` is SourceMod `blockhook`; SM/CSS/Swiftly nest rather than queue Give/Respawn/Terminate/SwitchTeam; `give`/`switchTeam`/`create`/`slay` must mutate before return; `respawn`/`terminateRound` are *documented* as queued today (not a “world changed” contract); give-from-`onCanAcquire` stays skip-and-named; proving slice is one command that asserts both give→CanAcquire and respawn→`onPre('player_spawn')`; `.d.ts` holes (missing re-entry notes; stale `switchTeam` “this frame” wording).

**The report’s opener and its own uncertainties disagree on mechanism.** The opener says the legal nest is “release `HOST` for the FFI duration and re-acquire it.” The coverage section then says: today’s natives cannot drop a `RefMut` that lives in the caller’s stack frame; no source describes releasing `HOST` across `engine_call_invoke`; `#63` did not evaluate stashing the outbound `FunctionCallbackInfo` for inbound `CallbackScope`; a raw C++ `HandleScope(Isolate*)` is still UB on the Rust aliasing axis even though V8 permits nested scopes.

So **spine A is the product goal, not a proven Rust patch.** Two candidate implementations remain, both unspiked:

1. **Refactor JS entry** so the outer `HOST` borrow is not live across `engine_call_invoke` (the report’s slogan). Requires changing `dispatch_onframe` / `fan_out_inner` / `eval` / `frame_async_drain`, not the native.
2. **Publish the outbound native’s live V8 handle** and build inbound `CallbackScope` without taking `HOST` (the nest study’s door). `#63` closed the no-handle C-ABI case only.

A spike that does not produce a compiling nested `dispatch_hook` under a JS `Engine.call` has not started the slice.

**Also Partial:** ModSharp’s give/respawn/terminate path was not re-opened in that run (earlier session source still stands: virtual call, optional `force` on the view overload). TTT cited from our specs, not edgegamers sources. `Events.fire` from a handler may already be synchronous and not a HOST hole (ddqgate). Cookie/ws/net/topmenu `try_borrow_mut` after drain not proven reachable.
