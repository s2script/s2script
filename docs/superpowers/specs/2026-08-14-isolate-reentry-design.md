# Isolate re-entry — outbound nest (board-wide)

**Status:** Locked 2026-08-14. Implementing.
**Audience:** core/shim + `@s2script/cs2` maintainers.
**Builds on:** isolate-reentry research; deferred-dispatch queue (#63); pickup-gates (PR1 thunk is independent); switchteam / respawn / terminateRound designs (those *queues* are revoked).

---

## 1. Rule

A plugin outbound engine call is visible to other plugins’ hooks and events **before that call returns**. SourceMod / CSS / Swiftly nest. We do the same under V8.

The isolate lock is an implementation accident. Anything we wrote to make that accident look like a product rule is torn out in this slice — not left as a “documented limitation.”

## 2. Mechanism

Outbound JS natives already have a live `FunctionCallbackInfo` on the stack. rusty_v8 149.4.0 can build `CallbackScope` from that handle **without** taking `HOST` (`promise_reject_cb` already does this).

1. Push `*const FunctionCallbackInfo` on a TLS stack for the FFI window (`engine_call_invoke`, `event_fire`, and any other JS native that calls the engine).
2. `fan_out_inner`: if the stack is non-empty, `CallbackScope` + per-subscriber `ContextScope`. **Do not** `try_borrow_mut` `HOST`.
3. If the stack is empty (true engine inbound, or C-ABI with no published handle): today’s `try_borrow_mut`. Notify may `Deferred`; pre-hooks skip and stay named.

`#63` stays for the no-handle case. We do **not** reconstruct `&mut Isolate` from a raw pointer.

Same-hook re-entry (`dispatch_hook` while `ACTIVE_HOOK` is that same owner+name) is **skip and name**. That is give-from-inside-`onCanAcquire` without a game-specific latch. Nest depth cap 8 as a backstop.

## 3. What is ripped out

| Goes away | Replacement |
|---|---|
| `pawn.js` `nextFrame` drains for `respawn` / `terminateRound` | Sync `Engine.call`, like SourceMod |
| “`onPre` can never run for JS-issued respawn/terminate — permanent” | Pre-hooks run on the nested inbound |
| `switchTeam` “events do not re-dispatch on that frame” | They do |
| `fakeCommand` “JS handler will not run — isolate borrow” | It runs (if the fake goes through a published native) |
| Fixtures that treat “JS respawn did not fire `onPre`” as success | Missing handler is a fail, except `bypassWith` |
| “Queued so events fire” / “lost to the isolate borrow” in `.d.ts` | SourceMod’s rule: the world has changed when this returns |

Author-facing `nextTick` / `nextFrame` stay.

## 4. What stays (real product rules)

- `bypassWith` on `onTerminateRound` / `onRespawn` — SourceMod `blockhook`. Other plugins still see `round_end` / `player_spawn`.
- Same-hook skip (give inside `onCanAcquire`).
- `await` illegal in a pre-hook.
- `#63` for engine-originated inbound with no V8 handle.
- Unload / ledger at a frame boundary.
- cookies / ws / net drains after JS has actually finished.

## 5. Acceptance

- Unit: JS `Engine.call` whose mock invoke `dispatch_hook`s a **different** hook → handler runs (`__ran === 1`).
- Unit: same-hook re-entry → skip + `status` contains `re-entrant` / `UNHOOKED`.
- Unit: `dispatch_hook` with empty nest stack while `HOST` held → still skip + named.
- `Player.respawn()` / `GameRules.terminateRound()` are synchronous; pending queues deleted.
- `.d.ts` no longer advertises those queues or the “onPre cannot run” story.
- Live-gate (when a CS2 box is available): `giveNamedItem` runs `onCanAcquire`; `respawn` runs `Events.onPre("player_spawn")`.

Pickup-gates PR1 is the thunk + subscribe API. JS give lighting it up is this host change.

## 6. Out

CanUse/CanEquip. Upgrading rusty_v8. Making `HOST` reentrant. Secret queues as a fallback if the spike fails — stop and re-open, do not queue.
